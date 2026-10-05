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
    ("file_open", 1, 1001, 14),
    ("registry_key_create", 1, 201001, 1),
    ("registry_key_delete", 1, 201001, 4),
    ("registry_key_rename", 1, 201001, 5),
    ("registry_key_create_unresolved", 1, 201001, 1),
    ("registry_value_set", 1, 201002, 2),
    ("registry_value_set_read_after", 1, 201002, 2),
    ("registry_value_set_unavailable", 1, 201002, 2),
    ("registry_value_delete", 1, 201002, 4),
    ("dns_response", 4, 4003, 2),
    ("event_log_stop", 1, 1008, 7),
    ("event_log_restart", 1, 1008, 8),
    ("event_log_disable", 1, 1008, 10),
    // Atlas extension 500, category 6 (Application Activity), class 1 (sensor spec §10.3).
    ("sensor_health_report", 6, 50_006_001, 1),
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
    assert_eq!(ids("event_log_disable").type_uid(), 100_810);
    // Above u32::MAX: `type_uid` is a u64.
    assert_eq!(ids("sensor_health_report").type_uid(), 5_000_600_101);
}

#[test]
fn sensor_health_class_uid_follows_the_ocsf_extension_rule() {
    // extension_uid × 100000 + category_uid × 1000 + n
    assert_eq!(atlas_schema::SENSOR_HEALTH_CLASS_UID, atlas_schema::ATLAS_EXTENSION_UID * 100_000 + 6 * 1000 + 1);
}
