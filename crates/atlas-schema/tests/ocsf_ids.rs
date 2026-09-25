//! Derived OCSF ids must equal OCSF 1.9.0 (spec 5.0).

mod common;

/// (sample, category_uid, class_uid, activity_id) copied from spec 5.0.
const EXPECTED: &[(&str, u32, u32, u32)] = &[
    ("process_launch", 1, 1007, 1),
    ("process_terminate", 1, 1007, 2),
    ("module_load", 1, 1005, 1),
    ("network_open", 4, 4001, 1),
    ("network_close", 4, 4001, 2),
    ("file_create", 1, 1001, 1),
    ("file_read", 1, 1001, 2),
    ("file_update", 1, 1001, 3),
    ("file_delete", 1, 1001, 4),
    ("file_rename", 1, 1001, 5),
    ("file_set_attributes", 1, 1001, 6),
    ("registry_key_create", 1, 201001, 1),
    ("registry_key_delete", 1, 201001, 4),
    ("registry_key_rename", 1, 201001, 5),
    ("registry_value_set", 1, 201002, 2),
    ("registry_value_delete", 1, 201002, 4),
    ("dns_response", 4, 4003, 2),
];

#[test]
fn derived_ids_match_ocsf_1_9_0() {
    let samples = common::samples();
    assert_eq!(samples.len(), EXPECTED.len(), "every sample needs an expected row");
    for (name, event) in samples {
        let &(_, category, class, activity) =
            EXPECTED.iter().find(|row| row.0 == name).unwrap_or_else(|| panic!("no expected ids for {name}"));
        let ids = event.kind.ocsf_ids();
        assert_eq!((ids.category_uid, ids.class_uid, ids.activity_id), (category, class, activity), "{name}");
        assert_eq!(ids.type_uid(), u64::from(class) * 100 + u64::from(activity), "{name}");
    }
}

#[test]
fn type_uid_examples() {
    let ids = |name: &str| common::samples().into_iter().find(|(n, _)| *n == name).unwrap().1.kind.ocsf_ids();
    assert_eq!(ids("process_launch").type_uid(), 100_701);
    assert_eq!(ids("registry_value_set").type_uid(), 20_100_202);
}
