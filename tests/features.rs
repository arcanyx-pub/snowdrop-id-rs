//! Tests for the `tokio` and `serde` feature surfaces. These compile only
//! when the corresponding features are enabled.

#[cfg(feature = "tokio")]
mod tokio_feature {
    use snowdrop_id::{Generator, MachineId};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn generate_async_is_unique_and_monotonic_per_task() {
        let generator = Arc::new(Generator::new(MachineId::new(3).unwrap()));
        let tasks = 8;
        let per_task = 5_000;

        let handles: Vec<_> = (0..tasks)
            .map(|_| {
                let generator = Arc::clone(&generator);
                tokio::spawn(async move {
                    let mut ids = Vec::with_capacity(per_task);
                    let mut last = None;
                    for _ in 0..per_task {
                        let id = generator.generate_async().await.unwrap();
                        if let Some(prev) = last {
                            assert!(id > prev);
                        }
                        last = Some(id);
                        ids.push(id.as_u64());
                    }
                    ids
                })
            })
            .collect();

        let mut all = std::collections::HashSet::new();
        for handle in handles {
            for id in handle.await.unwrap() {
                assert!(all.insert(id), "duplicate ID");
            }
        }
        assert_eq!(all.len(), tasks * per_task);
    }
}

#[cfg(feature = "serde")]
mod serde_feature {
    use snowdrop_id::{MachineId, SnowdropId};

    fn sample() -> SnowdropId {
        SnowdropId::from_parts(46_112_984, MachineId::new(0).unwrap(), 0).unwrap()
    }

    #[test]
    fn serializes_as_base62_string() {
        assert_eq!(serde_json::to_string(&sample()).unwrap(), "\"37U5o\"");
    }

    #[test]
    fn deserializes_from_base62_string() {
        let id: SnowdropId = serde_json::from_str("\"37U5o\"").unwrap();
        assert_eq!(id, sample());
        assert!(serde_json::from_str::<SnowdropId>("\"0abc\"").is_err());
        assert!(serde_json::from_str::<SnowdropId>("42").is_err());
    }

    #[test]
    fn numeric_representation_via_serde_u64() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Row {
            #[serde(with = "snowdrop_id::serde_u64")]
            id: SnowdropId,
        }

        let json = serde_json::to_string(&Row { id: sample() }).unwrap();
        assert_eq!(json, format!("{{\"id\":{}}}", sample().as_u64()));
        let row: Row = serde_json::from_str(&json).unwrap();
        assert_eq!(row.id, sample());

        // Bit 63 set is rejected.
        assert!(serde_json::from_str::<Row>("{\"id\":9223372036854775808}").is_err());
    }
}
