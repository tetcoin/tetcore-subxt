// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of tetcore-subxt.
//
// subxt is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// subxt is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with tetcore-subxt.  If not, see <http://www.gnu.org/licenses/>.

//! Implements support for the noble_contracts module.

use crate::frame::{
    balances::{
        Balances,
        BalancesEventsDecoder,
    },
    system::{
        System,
        SystemEventsDecoder,
    },
};
use codec::{
    Decode,
    Encode,
};
use core::marker::PhantomData;

/// Gas units are chosen to be represented by u64 so that gas metering
/// instructions can operate on them efficiently.
pub type Gas = u64;

/// The subset of the `noble_contracts::Trait` that a client must implement.
#[module]
pub trait Contracts: System + Balances {}

/// Stores the given binary Wasm code into the chain's storage and returns
/// its `codehash`.
/// You can instantiate contracts only with stored code.
#[derive(Clone, Debug, Eq, PartialEq, Call, Encode)]
pub struct PutCodeCall<'a, T: Contracts> {
    /// Runtime marker.
    pub _runtime: PhantomData<T>,
    /// Wasm blob.
    pub code: &'a [u8],
}

/// Creates a new contract from the `codehash` generated by `put_code`,
/// optionally transferring some balance.
///
/// Creation is executed as follows:
///
/// - The destination address is computed based on the sender and hash of
/// the code.
/// - The smart-contract account is instantiated at the computed address.
/// - The `ctor_code` is executed in the context of the newly-instantiated
/// account. Buffer returned after the execution is saved as the `code`https://www.bbc.co.uk/
/// of the account. That code will be invoked upon any call received by
/// this account.
/// - The contract is initialized.
#[derive(Clone, Debug, Eq, PartialEq, Call, Encode)]
pub struct InstantiateCall<'a, T: Contracts> {
    /// Initial balance transfered to the contract.
    #[codec(compact)]
    pub endowment: <T as Balances>::Balance,
    /// Gas limit.
    #[codec(compact)]
    pub gas_limit: Gas,
    /// Code hash returned by the put_code call.
    pub code_hash: &'a <T as System>::Hash,
    /// Data to initialize the contract with.
    pub data: &'a [u8],
}

/// Makes a call to an account, optionally transferring some balance.
///
/// * If the account is a smart-contract account, the associated code will
///  be executed and any value will be transferred.
/// * If the account is a regular account, any value will be transferred.
/// * If no account exists and the call value is not less than
/// `existential_deposit`, a regular account will be created and any value
///  will be transferred.
#[derive(Clone, Debug, PartialEq, Call, Encode)]
pub struct CallCall<'a, T: Contracts> {
    /// Address of the contract.
    pub dest: &'a <T as System>::Address,
    /// Value to transfer to the contract.
    #[codec(compact)]
    pub value: <T as Balances>::Balance,
    /// Gas limit.
    #[codec(compact)]
    pub gas_limit: Gas,
    /// Data to send to the contract.
    pub data: &'a [u8],
}

/// Code stored event.
#[derive(Clone, Debug, Eq, PartialEq, Event, Decode)]
pub struct CodeStoredEvent<T: Contracts> {
    /// Code hash of the contract.
    pub code_hash: T::Hash,
}

/// Instantiated event.
#[derive(Clone, Debug, Eq, PartialEq, Event, Decode)]
pub struct InstantiatedEvent<T: Contracts> {
    /// Caller that instantiated the contract.
    pub caller: <T as System>::AccountId,
    /// The address of the contract.
    pub contract: <T as System>::AccountId,
}

/// Contract execution event.
///
/// Emitted upon successful execution of a contract, if any contract events were produced.
#[derive(Clone, Debug, Eq, PartialEq, Event, Decode)]
pub struct ContractExecutionEvent<T: Contracts> {
    /// Caller of the contract.
    pub caller: <T as System>::AccountId,
    /// SCALE encoded contract event data.
    pub data: Vec<u8>,
}

#[cfg(test)]
#[cfg(feature = "integration-tests")]
mod tests {
    use sp_keyring::AccountKeyring;

    use super::*;
    use crate::{
        balances::*,
        system::*,
        Client,
        ClientBuilder,
        ContractsTemplateRuntime,
        Error,
        ExtrinsicSuccess,
        PairSigner,
        Signer,
    };
    use sp_core::{
        crypto::AccountId32,
        sr25519::Pair,
    };
    use std::sync::atomic::{
        AtomicU32,
        Ordering,
    };

    static STASH_NONCE: std::sync::atomic::AtomicU32 = AtomicU32::new(0);

    struct TestContext {
        client: Client<ContractsTemplateRuntime>,
        signer: PairSigner<ContractsTemplateRuntime, Pair>,
    }

    impl TestContext {
        async fn init() -> Self {
            env_logger::try_init().ok();

            let client = ClientBuilder::<ContractsTemplateRuntime>::new()
                .build()
                .await
                .expect("Error creating client");
            let mut stash = PairSigner::new(AccountKeyring::Alice.pair());
            let nonce = client
                .account(&stash.account_id(), None)
                .await
                .unwrap()
                .nonce;
            let local_nonce = STASH_NONCE.fetch_add(1, Ordering::SeqCst);

            stash.set_nonce(nonce + local_nonce);

            let signer = Self::generate_account(&client, &mut stash).await;

            TestContext { client, signer }
        }

        /// generate a new keypair for an account, and fund it so it can perform smart contract operations
        async fn generate_account(
            client: &Client<ContractsTemplateRuntime>,
            stash: &mut PairSigner<ContractsTemplateRuntime, Pair>,
        ) -> PairSigner<ContractsTemplateRuntime, Pair> {
            use sp_core::Pair as _;
            let new_account = Pair::generate().0;
            let new_account_id: AccountId32 = new_account.public().into();
            // fund the account
            let endowment = 200_000_000_000_000;
            let _ = client
                .transfer_and_watch(stash, &new_account_id, endowment)
                .await
                .expect("New account balance transfer failed");
            stash.increment_nonce();
            PairSigner::new(new_account)
        }

        async fn put_code(
            &self,
        ) -> Result<CodeStoredEvent<ContractsTemplateRuntime>, Error> {
            const CONTRACT: &str = r#"
                (module
                    (func (export "call"))
                    (func (export "deploy"))
                )
            "#;
            let code = wabt::wat2wasm(CONTRACT).expect("invalid wabt");

            let result = self.client.put_code_and_watch(&self.signer, &code).await?;
            let code_stored = result.code_stored()?.ok_or_else(|| {
                Error::Other("Failed to find a CodeStored event".into())
            })?;
            log::info!("Code hash: {:?}", code_stored.code_hash);
            Ok(code_stored)
        }

        async fn instantiate(
            &self,
            code_hash: &<ContractsTemplateRuntime as System>::Hash,
            data: &[u8],
        ) -> Result<InstantiatedEvent<ContractsTemplateRuntime>, Error> {
            // call instantiate extrinsic
            let result = self
                .client
                .instantiate_and_watch(
                    &self.signer,
                    100_000_000_000_000, // endowment
                    500_000_000,         // gas_limit
                    code_hash,
                    data,
                )
                .await?;

            log::info!("Instantiate result: {:?}", result);
            let instantiated = result.instantiated()?.ok_or_else(|| {
                Error::Other("Failed to find a Instantiated event".into())
            })?;

            Ok(instantiated)
        }

        async fn call(
            &self,
            contract: &<ContractsTemplateRuntime as System>::Address,
            input_data: &[u8],
        ) -> Result<ExtrinsicSuccess<ContractsTemplateRuntime>, Error> {
            let result = self
                .client
                .call_and_watch(
                    &self.signer,
                    contract,
                    0,           // value
                    500_000_000, // gas_limit
                    input_data,
                )
                .await?;
            log::info!("Call result: {:?}", result);
            Ok(result)
        }
    }

    #[async_std::test]
    async fn tx_put_code() {
        let ctx = TestContext::init().await;
        let code_stored = ctx.put_code().await;

        assert!(
            code_stored.is_ok(),
            format!(
                "Error calling put_code and receiving CodeStored Event: {:?}",
                code_stored
            )
        );
    }

    #[async_std::test]
    async fn tx_instantiate() {
        let ctx = TestContext::init().await;
        let code_stored = ctx.put_code().await.unwrap();

        let instantiated = ctx.instantiate(&code_stored.code_hash, &[]).await;

        assert!(
            instantiated.is_ok(),
            format!("Error instantiating contract: {:?}", instantiated)
        );
    }

    #[async_std::test]
    async fn tx_call() {
        let ctx = TestContext::init().await;
        let code_stored = ctx.put_code().await.unwrap();

        let instantiated = ctx.instantiate(&code_stored.code_hash, &[]).await.unwrap();
        let executed = ctx.call(&instantiated.contract, &[]).await;

        assert!(
            executed.is_ok(),
            format!("Error calling contract: {:?}", executed)
        );
    }
}
