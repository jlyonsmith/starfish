#![cfg(feature = "tabled")]
use starfish_db::{Host, HostGroupUser, User};
use tabled::Table;

#[test]
fn smoke() {
    let u = User {
        id: 1,
        alias: "jls".into(),
        email: "j@example.com".into(),
        first_name: "John".into(),
        last_name: "Lyon-Smith".into(),
        ssh_keys: Default::default(),
        host_group_users: Default::default(),
        host_groups: Default::default(),
        updated_at: jiff::Timestamp::UNIX_EPOCH,
        created_at: jiff::Timestamp::UNIX_EPOCH,
    };
    println!("{}", Table::new(vec![u]));

    let h = Host {
        id: 1,
        host_group_id: 2,
        host_group: Default::default(),
        hostname: "web1".into(),
        info: "".into(),
        agent_key: "abcd".into(),
        contacted_at: None,
        next_heartbeat_at: Some(jiff::Timestamp::UNIX_EPOCH),
        created_at: jiff::Timestamp::UNIX_EPOCH,
        updated_at: jiff::Timestamp::UNIX_EPOCH,
    };
    println!("{}", Table::new(vec![h]));

    // `security_groups` is the one column with a hand written display, because
    // `Vec<String>` has no `Display` of its own.
    let m = HostGroupUser {
        host_group_id: 2,
        host_group: Default::default(),
        user_id: 1,
        user: Default::default(),
        is_sudoer: true,
        security_groups: vec!["deploy".into(), "developers".into()],
        created_at: jiff::Timestamp::UNIX_EPOCH,
        updated_at: jiff::Timestamp::UNIX_EPOCH,
    };
    assert!(
        Table::new(vec![m])
            .to_string()
            .contains("deploy, developers")
    );
}
