// Copyright (c) The Diem Core Contributors
// SPDX-License-Identifier: Apache-2.0

use aptos_transaction_builder::aptos_stdlib;
use forge::{AptosContext, AptosTest, Result, Test};

pub struct MintTransfer;

impl Test for MintTransfer {
    fn name(&self) -> &'static str {
        "smoke-test::aptos::mint-transfer"
    }
}

#[async_trait::async_trait]
impl AptosTest for MintTransfer {
    async fn run<'t>(&self, ctx: &mut AptosContext<'t>) -> Result<()> {
        let mut account1 = ctx.random_account();
        ctx.create_user_account(account1.public_key()).await?;
        let account2 = ctx.random_account();
        ctx.create_user_account(account2.public_key()).await?;

        ctx.mint(account1.address(), 10000).await?;

        let transfer_txn =
            account1.sign_with_transaction_builder(ctx.aptos_transaction_factory().payload(
                aptos_stdlib::encode_transfer_script_function(account2.address(), 400),
            ));
        ctx.client().submit_and_wait(&transfer_txn).await?;

        // test delegation
        let txn_factory = ctx.aptos_transaction_factory();
        let delegate_txn1 = ctx
            .root_account()
            .sign_with_transaction_builder(txn_factory.payload(
                aptos_stdlib::encode_delegate_mint_capability_script_function(account1.address()),
            ));
        ctx.client().submit_and_wait(&delegate_txn1).await?;

        // Test delegating more than one at a time: faucet startup stampeding herd
        let delegate_txn2 = ctx
            .root_account()
            .sign_with_transaction_builder(txn_factory.payload(
                aptos_stdlib::encode_delegate_mint_capability_script_function(account2.address()),
            ));
        ctx.client().submit_and_wait(&delegate_txn2).await?;

        let claim_txn = account1.sign_with_transaction_builder(
            txn_factory.payload(aptos_stdlib::encode_claim_mint_capability_script_function()),
        );
        ctx.client().submit_and_wait(&claim_txn).await?;
        let mint_txn = account1.sign_with_transaction_builder(txn_factory.payload(
            aptos_stdlib::encode_mint_script_function(account1.address(), 100),
        ));
        ctx.client().submit_and_wait(&mint_txn).await?;

        Ok(())
    }
}
