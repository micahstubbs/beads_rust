//! Property-based tests for model serde round-trip and content_hash stability.
//!
//! Verifies that Status and IssueType deserialize case-insensitively,
//! round-trip correctly through JSON, and produce stable content hashes
//! regardless of input casing.

use proptest::prelude::*;

use beads_rust::model::{IssueType, Status};
use beads_rust::util::content_hash_from_parts;

fn arb_status_name() -> impl Strategy<Value = (&'static str, Status)> {
    prop_oneof![
        Just(("open", Status::Open)),
        Just(("in_progress", Status::InProgress)),
        Just(("blocked", Status::Blocked)),
        Just(("deferred", Status::Deferred)),
        Just(("draft", Status::Draft)),
        Just(("closed", Status::Closed)),
        Just(("tombstone", Status::Tombstone)),
        Just(("pinned", Status::Pinned)),
    ]
}

fn arb_issue_type_name() -> impl Strategy<Value = (&'static str, IssueType)> {
    prop_oneof![
        Just(("task", IssueType::Task)),
        Just(("bug", IssueType::Bug)),
        Just(("feature", IssueType::Feature)),
        Just(("epic", IssueType::Epic)),
        Just(("chore", IssueType::Chore)),
        Just(("docs", IssueType::Docs)),
        Just(("question", IssueType::Question)),
    ]
}

fn case_variants(s: &str) -> Vec<String> {
    vec![
        s.to_string(),
        s.to_uppercase(),
        {
            let mut c = s.chars();
            match c.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
            }
        },
        s.chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                }
            })
            .collect(),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn status_deser_case_insensitive((canonical, expected) in arb_status_name()) {
        for variant in case_variants(canonical) {
            let json = format!("\"{}\"", variant);
            let deserialized: Status = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("failed to deser Status from {json}: {e}"));
            prop_assert_eq!(
                &deserialized, &expected,
                "Status '{}' should deser to {:?}, got {:?}",
                variant, expected, deserialized
            );
        }
    }

    #[test]
    fn issue_type_deser_case_insensitive((canonical, expected) in arb_issue_type_name()) {
        for variant in case_variants(canonical) {
            let json = format!("\"{}\"", variant);
            let deserialized: IssueType = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("failed to deser IssueType from {json}: {e}"));
            prop_assert_eq!(
                &deserialized, &expected,
                "IssueType '{}' should deser to {:?}, got {:?}",
                variant, expected, deserialized
            );
        }
    }

    #[test]
    fn status_roundtrip((_, expected) in arb_status_name()) {
        let serialized = serde_json::to_string(&expected).unwrap();
        let deserialized: Status = serde_json::from_str(&serialized).unwrap();
        prop_assert_eq!(&deserialized, &expected);
    }

    #[test]
    fn issue_type_roundtrip((_, expected) in arb_issue_type_name()) {
        let serialized = serde_json::to_string(&expected).unwrap();
        let deserialized: IssueType = serde_json::from_str(&serialized).unwrap();
        prop_assert_eq!(&deserialized, &expected);
    }

    #[test]
    fn content_hash_stable_across_status_casing(
        (canonical, expected) in arb_status_name(),
        title in "[a-z]{1,20}",
    ) {
        let reference_hash = content_hash_from_parts(
            &title, None, None, None, None,
            &expected, &beads_rust::model::Priority::MEDIUM,
            &IssueType::Task, None, None, None, None, None, false, false,
        );
        for variant in case_variants(canonical) {
            let json = format!("\"{}\"", variant);
            let status: Status = serde_json::from_str(&json).unwrap();
            let hash = content_hash_from_parts(
                &title, None, None, None, None,
                &status, &beads_rust::model::Priority::MEDIUM,
                &IssueType::Task, None, None, None, None, None, false, false,
            );
            prop_assert_eq!(
                &hash, &reference_hash,
                "Hash mismatch for status variant '{}': {} vs {}",
                variant, hash, reference_hash
            );
        }
    }

    #[test]
    fn content_hash_stable_across_issue_type_casing(
        (canonical, expected) in arb_issue_type_name(),
        title in "[a-z]{1,20}",
    ) {
        let reference_hash = content_hash_from_parts(
            &title, None, None, None, None,
            &Status::Open, &beads_rust::model::Priority::MEDIUM,
            &expected, None, None, None, None, None, false, false,
        );
        for variant in case_variants(canonical) {
            let json = format!("\"{}\"", variant);
            let issue_type: IssueType = serde_json::from_str(&json).unwrap();
            let hash = content_hash_from_parts(
                &title, None, None, None, None,
                &Status::Open, &beads_rust::model::Priority::MEDIUM,
                &issue_type, None, None, None, None, None, false, false,
            );
            prop_assert_eq!(
                &hash, &reference_hash,
                "Hash mismatch for issue_type variant '{}': {} vs {}",
                variant, hash, reference_hash
            );
        }
    }

    #[test]
    fn custom_status_preserved_verbatim(name in "[a-z_]{3,15}") {
        let known = ["open", "in_progress", "inprogress", "blocked", "deferred",
                      "draft", "closed", "tombstone", "pinned"];
        prop_assume!(!known.contains(&name.as_str()));
        let json = format!("\"{name}\"");
        let status: Status = serde_json::from_str(&json).unwrap();
        match &status {
            Status::Custom(v) => prop_assert_eq!(v, &name),
            other => prop_assert!(false, "expected Custom, got {:?}", other),
        }
    }

    #[test]
    fn custom_issue_type_preserved_verbatim(name in "[a-z_]{3,15}") {
        let known = ["task", "bug", "feature", "epic", "chore", "docs", "question"];
        prop_assume!(!known.contains(&name.as_str()));
        let json = format!("\"{name}\"");
        let issue_type: IssueType = serde_json::from_str(&json).unwrap();
        match &issue_type {
            IssueType::Custom(v) => prop_assert_eq!(v, &name),
            other => prop_assert!(false, "expected Custom, got {:?}", other),
        }
    }
}
