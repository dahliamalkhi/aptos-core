script {
    use DiemFramework::AccountFreezing;
    use DiemFramework::ChainId;
    use DiemFramework::XUS;
    use DiemFramework::DualAttestation;
    use DiemFramework::XDX;
    use DiemFramework::Diem;
    use DiemFramework::DiemAccount;
    use DiemFramework::Block;
    use DiemFramework::Reconfiguration;
    use DiemFramework::ValidatorSystem;
    use DiemFramework::Timestamp;
    use DiemFramework::TransactionPublishingOption;
    use DiemFramework::Version;
    use DiemFramework::TransactionFee;
    use DiemFramework::VMConfig;
    use Std::Vector;

    fun genesis(
        dr_account: signer,
        tc_account: signer,
    ) {
        let dr_account = &dr_account;
        let tc_account = &tc_account;
        let dummy_auth_key = x"0000000000000000000000000000000000000000000000000000000000000000";
        let dr_auth_key = copy dummy_auth_key;
        let tc_auth_key = dummy_auth_key;

        // no script allowlist + allow open publishing
        let initial_script_allow_list = Vector::empty();
        let is_open_module = true;
        let instruction_schedule = Vector::empty();
        let native_schedule = Vector::empty();
        let chain_id = 0;
        let initial_diem_version = 1;

        DiemAccount::initialize(dr_account, x"00000000000000000000000000000000");

        ChainId::initialize(dr_account, chain_id);

        // On-chain config setup
        Reconfiguration::initialize(dr_account);

        // Currency setup
        Diem::initialize(dr_account);

        // Currency setup
        XUS::initialize(dr_account, tc_account);

        XDX::initialize(
            dr_account,
            tc_account,
        );

        AccountFreezing::initialize(dr_account);

        TransactionFee::initialize(tc_account);

        ValidatorSystem::initialize_validator_set(dr_account);
        Version::initialize(dr_account, initial_diem_version);
        DualAttestation::initialize(dr_account);
        Block::initialize_block_metadata(dr_account);

        let dr_rotate_key_cap = DiemAccount::extract_key_rotation_capability(dr_account);
        DiemAccount::rotate_authentication_key(&dr_rotate_key_cap, dr_auth_key);
        DiemAccount::restore_key_rotation_capability(dr_rotate_key_cap);

        TransactionPublishingOption::initialize(
            dr_account,
            initial_script_allow_list,
            is_open_module,
        );

        VMConfig::initialize(
            dr_account,
            instruction_schedule,
            native_schedule,
        );

        let tc_rotate_key_cap = DiemAccount::extract_key_rotation_capability(tc_account);
        DiemAccount::rotate_authentication_key(&tc_rotate_key_cap, tc_auth_key);
        DiemAccount::restore_key_rotation_capability(tc_rotate_key_cap);
        Timestamp::set_time_has_started_for_testing(dr_account);
    }

}
