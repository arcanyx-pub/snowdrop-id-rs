//! The normative test vectors from SPEC.md §9, verified in both directions.

use snowdrop_id::{Epoch, MachineId, SnowdropId};

struct Vector {
    ms: u64,
    timestamp: u32,
    machine: u16,
    seq: u32,
    id: u64,
    base62: &'static str,
}

const VECTORS: &[Vector] = &[
    Vector {
        ms: 0,
        timestamp: 0,
        machine: 0,
        seq: 0,
        id: 0,
        base62: "0",
    },
    Vector {
        ms: 1024,
        timestamp: 1,
        machine: 0,
        seq: 0,
        id: 4294967296,
        base62: "1",
    },
    Vector {
        ms: 47219696000,
        timestamp: 46112984,
        machine: 0,
        seq: 0,
        id: 198053758200971264,
        base62: "37U5o",
    },
    Vector {
        ms: 47219696000,
        timestamp: 46112984,
        machine: 0,
        seq: 1,
        id: 198053758200971265,
        base62: "ciNixiq",
    },
    Vector {
        ms: 47219696000,
        timestamp: 46112984,
        machine: 1,
        seq: 0,
        id: 198053758205165568,
        base62: "2OS6gq",
    },
    Vector {
        ms: 47219696000,
        timestamp: 46112984,
        machine: 25,
        seq: 0,
        id: 198053758305828864,
        base62: "weR31c",
    },
    Vector {
        ms: 47219696000,
        timestamp: 46112984,
        machine: 613,
        seq: 12345,
        id: 198053760772091961,
        base62: "20L2JaLj3o",
    },
    Vector {
        ms: 2199023254528,
        timestamp: 2147483647,
        machine: 1023,
        seq: 4194303,
        id: 9223372036854775807,
        base62: "AzL8n0Y58m7",
    },
];

#[test]
fn spec_vectors_encode() {
    for v in VECTORS {
        let mid = MachineId::new(v.machine).unwrap();
        assert_eq!(
            v.ms >> 10,
            v.timestamp as u64,
            "ms→timestamp for {}",
            v.base62
        );
        let id = SnowdropId::from_parts(v.timestamp, mid, v.seq).unwrap();
        assert_eq!(id.as_u64(), v.id, "assembled ID for {}", v.base62);
        assert_eq!(id.encode(), v.base62, "encoding of {}", v.id);
    }
}

#[test]
fn spec_vectors_decode() {
    for v in VECTORS {
        let id = SnowdropId::decode(v.base62).unwrap();
        assert_eq!(id.as_u64(), v.id, "decoded value of {}", v.base62);
        assert_eq!(id.timestamp(), v.timestamp);
        assert_eq!(id.machine_id().get(), v.machine);
        assert_eq!(id.sequence(), v.seq);
        assert_eq!(
            id.window_start_ms(Epoch::DEFAULT),
            Epoch::DEFAULT.unix_ms() + (v.ms >> 10 << 10),
            "window start of {}",
            v.base62
        );
    }
}

#[test]
fn full_bijection_sweep() {
    // Deterministic pseudo-random 63-bit sweep: id → encode → decode → id.
    let mut x: u64 = 0x9E3779B97F4A7C15;
    for _ in 0..100_000 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let id = SnowdropId::from_u64(x >> 1).unwrap();
        let s = id.encode();
        assert!(s.len() <= 11);
        assert_eq!(SnowdropId::decode(&s).unwrap(), id);
    }
}
