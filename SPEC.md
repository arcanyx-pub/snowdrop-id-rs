# Snowdrop ID Specification

<p align="center">
  <img src="assets/snowdrop.jpg" alt="Snowdrop, the Heeler puppy mascot, holding a snowdrop flower in a snowy forest" width="480">
</p>

**Version:** 1.0 (draft 3)
**Date:** 2026-07-11
**Status:** Draft

This document specifies the Snowdrop ID format and its generation and
encoding algorithms. It is language-independent; `snowdrop-id` (Rust) is
the reference implementation.

The key words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and
**MAY** are to be interpreted as described in RFC 2119.

## 1. Introduction

A Snowdrop ID is a 63-bit unsigned integer identifier, similar in purpose
to a Twitter Snowflake ID:

- **BTree-friendly**: IDs are roughly monotonically increasing across a
  cluster and strictly increasing per generator, so database inserts
  append near the right edge of a primary-key BTree.
- **Collision-free**: with correctly assigned machine IDs, no two
  generators ever produce the same ID.
- **63-bit**: the value always fits in a signed 64-bit integer
  (`BIGINT`) with a zero sign bit.

Snowdrop IDs add two goals beyond Snowflake:

- **Short external form**: a field-reordering transform (§6) yields a
  base62 string that is 7 characters or fewer in the common case — as
  few as 5 — versus 11 characters for a naively base62-encoded
  Snowflake.
- **Snowflake interchangeability**: with a shared epoch, a Snowdrop ID
  and a Snowflake ID generated at the same instant are numerically equal
  to within one low-field's worth (< 2³²), so both families can be
  mixed in the same BTree keyspace with exact time ordering at ~1-second
  granularity (§7.3).

## 2. Notation

- Bits are numbered least-significant first: bit 0 is the 1's place,
  bit 63 is the most significant bit of the 64-bit word.
- `rev_n(x)` denotes the bitwise reversal of the low `n` bits of `x`:
  bit `i` moves to bit `n − 1 − i`, for `0 ≤ i < n`. `rev_n` is its own
  inverse.
- `x << k` / `x >> k` are logical shifts on 64-bit unsigned values.

## 3. ID Structure

A Snowdrop ID is a 64-bit word laid out as follows:

| Bits    | Width | Field      | Description                                    |
|---------|-------|------------|------------------------------------------------|
| 63      | 1     | reserved   | Always `0` (sign bit of a signed 64-bit int)   |
| 62 – 32 | 31    | timestamp  | Milliseconds since the epoch, `>> 10` (§4)     |
| 31 – 22 | 10    | machine ID | Generator identifier, `0 … 1023` (§5.1)        |
| 21 – 0  | 22    | sequence   | Per-window counter, `0 … 4,194,303` (§5.2)     |

Equivalently:

```
id = (timestamp << 32) | (machine_id << 22) | sequence
```

The reserved bit MUST be `0`. An ID with bit 63 set is not a valid
Snowdrop ID.

The timestamp is the number of milliseconds elapsed since the epoch,
right-shifted by 10 bits — i.e. it counts **1024 ms windows**:

```
timestamp = (unix_time_ms − epoch_ms) >> 10
```

This choice makes Snowdrop numerically interchangeable with Snowflake
(41-bit millisecond timestamp at bits 62–22): shifting a millisecond
count right by 10 and placing it 10 bits higher multiplies it back by
exactly the amount dropped, so both families scale identically (§7.3).
Sub-window ordering is provided by the sequence counter rather than the
clock.

## 4. Epoch

The timestamp field counts 1024 ms windows since a deployment-defined
**epoch**, expressed in milliseconds.

- The default epoch is **2026-01-01T00:00:00Z** (Unix time
  `1767225600000` ms). Implementations MUST use this epoch unless
  explicitly configured otherwise.
- The epoch MUST be configurable in milliseconds. All generators sharing
  an ID space MUST use the same epoch, and the epoch of an ID space MUST
  NOT change after IDs have been issued.
- 31 bits of 1024 ms windows give a range of ~69.7 years. With the
  default epoch, the timestamp field exhausts at
  **2095-09-07T15:47:35Z**.
- For Snowflake interchangeability (§7.3), configure the epoch to equal
  the Snowflake epoch already in use in the deployment (e.g. Twitter's
  `1288834974657`).

## 5. Generation Algorithm

### 5.1 Machine ID

The 10-bit machine ID distinguishes concurrent generators.

- Every concurrently active generator in an ID space MUST have a unique
  machine ID. Assignment is out of scope for this spec (static
  configuration, a coordination service, etc.).
- A "machine" is a **generator instance**, not a host: two processes
  generating IDs on the same host MUST use different machine IDs or
  share one coordinated generator.
- Machine IDs SHOULD be assigned from 0 upward: the string encoding
  (§6.4) is shorter for smaller machine IDs. Machine IDs 0–25 get
  6-character strings for the first ID of each window (5 characters for
  machine ID 0 until 2055); higher machine IDs get 7.

### 5.2 Generator state and algorithm

A generator holds two mutable state variables: `last_ts` (the timestamp
of the most recently issued ID, initially `−1`) and `seq` (the last
sequence number issued). State updates MUST be atomic with respect to
concurrent callers of the same generator: it must be impossible for two
calls to observe the same `(last_ts, seq)` pair.

```
generate():
    t ← (unix_time_ms() − epoch_ms) >> 10

    if t > 2³¹ − 1:
        fail permanently                    # epoch exhausted (§4)

    if t < last_ts:
        t ← last_ts                         # hold rule (§5.3)

    if t == last_ts:
        if seq == 2²² − 1:                  # sequence exhausted (§5.4)
            wait until the clock enters the window after last_ts
            t ← (unix_time_ms() − epoch_ms) >> 10
            seq ← 0
        else:
            seq ← seq + 1
    else:                                   # t > last_ts
        seq ← 0

    last_ts ← t
    return (t << 32) | (machine_id << 22) | seq
```

This yields **strict monotonicity per generator**: each ID is
numerically greater than the previous one, because either the timestamp
field increased, or it stayed equal and the sequence field increased.

### 5.3 Clock regression (hold rule)

Wall clocks can step backwards (NTP step corrections near boot,
hibernate/resume, VM migration, manual changes). When the observed clock
is behind `last_ts`, the generator MUST NOT emit a smaller timestamp.
Instead it MUST **hold**: continue issuing IDs under `last_ts`,
consuming sequence numbers, until the wall clock catches up. With
4,194,304 sequence values per held window, a hold of a few seconds
consumes negligible sequence space.

If the sequence exhausts while the clock is held (§5.4), the generator
blocks until the wall clock passes the end of the `last_ts` window.

Note that Snowdrop's 1024 ms granularity makes the hold rule engage
rarely: sub-second slew corrections never trip it.

**Restart hazard (non-normative).** The hold rule protects a *running*
generator. If a generator restarts while the clock is behind the
timestamps its previous incarnation issued, it can reissue old
`(timestamp, sequence)` pairs — the same hazard exists in Snowflake.
Deployments SHOULD mitigate by waiting for clock synchronization (e.g.,
NTP sync) before serving, and MAY persist `last_ts` across restarts and
apply the hold rule to the persisted value.

### 5.4 Sequence exhaustion

If more than 2²² IDs are requested within one timestamp window, the
generator MUST NOT reuse or wrap the sequence. It MUST wait until the
wall clock advances past the end of the `last_ts` window, then continue
with the new timestamp and `seq = 0`. (This is a sustained rate of
~4.09M IDs/second per generator, not expected in practice.)

### 5.5 Validity

A valid Snowdrop ID satisfies: bit 63 is `0`; `timestamp ≤ 2³¹ − 1`;
`machine_id ≤ 1023`; `sequence ≤ 2²² − 1`. All 63-bit values are
structurally valid; consumers cannot distinguish a Snowdrop ID from any
other 63-bit integer by inspection.

## 6. External String Representation

The external form is a base62 string of a **transformed** ID. The
transform swaps the field order — sequence and machine ID, which are
usually small, move to the top of the word; the timestamp, which is
always large, moves to the bottom — so the transformed integer, and
therefore its base62 representation, is small in the common case.

### 6.1 Alphabet

Base62 uses the following alphabet, in ascending digit-value order
(ASCII order):

```
0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz
```

Digit `0` has value 0; digit `z` has value 61. The most significant
digit is written first. The encoding of value 0 is the single character
`"0"`; otherwise the string MUST NOT have leading `0` characters.

### 6.2 Encoding

Given a valid ID `id` with fields `timestamp`, `machine_id`,
`sequence`, compute the transformed value `v`:

```
v = (sequence << 41) | (machine_id << 31) | timestamp
```

then encode `v` in base62 (§6.1), without leading zero digits. The
transformed layout is the ID's field order swapped, each field keeping
its original bit order:

| Bits    | Width | Content    |
|---------|-------|------------|
| 62 – 41 | 22    | sequence   |
| 40 – 31 | 10    | machine ID |
| 30 – 0  | 31    | timestamp  |

Equivalently, the transform can be computed with bit reversals:

1. **Reverse each field in place**:
   `step1 = (rev₃₁(timestamp) << 32) | (rev₁₀(machine_id) << 22) | rev₂₂(sequence)`
2. **Reverse the entire 64-bit word:** `step2 = rev₆₄(step1)`.
3. **Shift right by 1** to drop the reserved bit (always `0`), which the
   full reversal moved to bit 0: `v = step2 >> 1`.

Reversing a field in place and then reversing the whole word restores
the field's original bit order while relocating it to the mirrored
position, so steps 1–3 produce exactly the closed form above.
Implementations MAY use either formulation; they MUST produce identical
output.

**Why the swap works (non-normative).** The sequence resets every
window and the machine ID is small in well-configured deployments, so
the high bits of `v` are usually zero and the string is short: each
unset high bit saves string length. The timestamp — large from day one —
sits at the bottom, where its magnitude is bounded by 2³¹ and can never
push the string past 6 characters on its own. The transform is a
bijection on 63-bit values, so no information is lost.

### 6.3 Decoding

Given a base62 string `s`:

1. Interpret `s` as a base62 integer `v` (§6.1). Decoders MUST reject
   strings containing characters outside the alphabet, strings longer
   than 11 characters, values `v ≥ 2⁶³`, and the empty string. Decoders
   SHOULD reject non-canonical strings (leading `0` digits).
2. Extract the fields and reassemble:
   `sequence = v >> 41` ; `machine_id = (v >> 31) & 0x3FF` ;
   `timestamp = v & 0x7FFFFFFF` ;
   `id = (timestamp << 32) | (machine_id << 22) | sequence`.

Decoding is the exact inverse of encoding: `decode(encode(id)) = id` for
every valid ID.

### 6.4 String length

The string length is a deterministic function of the sequence, machine
ID, and date, in that order of dominance:

| Condition                        | Maximum length                       |
|----------------------------------|--------------------------------------|
| `sequence = 0`, `machine_id = 0` | 5 until 2055-09-23, then 6 (default epoch) |
| `sequence = 0`, `machine_id ≤ 25`| 6                                    |
| `sequence = 0`, any machine ID   | 7                                    |
| `sequence ≤ 98`                  | 8                                    |
| `sequence ≤ 6,154`               | 9                                    |
| `sequence ≤ 381,668`             | 10                                   |
| any valid ID                     | 11                                   |

The first ID issued in any given window therefore encodes to at most 7
characters — 6 for machine IDs 0–25, and 5 for machine ID 0 until 2055.
Under sustained load, length grows logarithmically with the per-window
ID rate: about 8 characters up to ~98 IDs/window, 9 up to ~6 k, 10 up to
~380 k per generator.

Note that base62 string ordering does **not** match numeric ID ordering
(nor generation order). The string form is an opaque external handle;
sorting and range queries MUST use the integer form.

## 7. Properties

### 7.1 Uniqueness

Within one ID space (shared epoch, unique machine IDs), every generated
ID is unique: IDs from different generators differ in the machine ID
field, and IDs from one generator differ in `(timestamp, sequence)` by
the strict monotonicity of §5.2.

### 7.2 Ordering

- **Per generator:** strictly increasing, in generation order.
- **Across generators:** ordered by timestamp window; within the same
  1024 ms window, ordering is by `(machine_id, sequence)` and does not
  reflect generation order. IDs are "roughly monotonic" cluster-wide
  with ~1-second granularity, which is sufficient for right-edge BTree
  append behavior.

### 7.3 Snowflake interchangeability

A Snowflake ID places a 41-bit millisecond timestamp at bits 62–22, so
its numeric value is `ms × 2²² + low`, where `low < 2²²` holds the
worker and sequence fields. A Snowdrop ID's value is

```
(ms >> 10) × 2³² + low  =  (ms − (ms mod 1024)) × 2²² + low
```

With a **shared epoch**, a Snowdrop ID and a Snowflake ID generated in
the same 1024 ms window differ by less than 2³², and IDs from different
windows order exactly by time. The two families interleave with perfect
write locality and exact cross-family time ordering at 1024 ms
granularity — a deployment can migrate from Snowflake to Snowdrop (or
run both) in a single keyspace without disturbing BTree append behavior.

Caveats (non-normative):

- Within one 1024 ms window, cross-family ordering is arbitrary (as it
  already is *within* each family at sequence granularity).
- Uniqueness is only guaranteed *within* each family. A Snowdrop ID can
  numerically collide with a Snowflake ID; deployments mixing both in
  one keyspace must ensure disjointness externally (e.g., migrate
  write traffic from one family to the other at a cutover time).

## 8. Security Considerations

Snowdrop IDs are not secrets and MUST NOT be used as capabilities or
authorization tokens. An ID reveals its creation time (to ~1 second),
the issuing machine ID, and the position in that window's sequence;
observing a stream of IDs reveals approximate generation rates. The
base62 transform is a reversible encoding, not obfuscation. Deployments
that need non-guessable or non-enumerable identifiers should use random
identifiers (e.g., UUIDv4) instead, or in addition.

## 9. Test Vectors

All vectors use the default epoch (2026-01-01T00:00:00Z =
`1767225600000` ms). The `ms` column is milliseconds since the epoch;
`timestamp = ms >> 10`. `ms = 47219696000` corresponds to
2027-07-01T12:34:56Z.

| ms            | timestamp  | machine | seq     | ID (decimal)        | ID (hex)             | transformed (hex)    | base62        |
|---------------|------------|---------|---------|---------------------|----------------------|----------------------|---------------|
| 0             | 0          | 0       | 0       | 0                   | `0x0000000000000000` | `0x0000000000000000` | `0`           |
| 1024          | 1          | 0       | 0       | 4294967296          | `0x0000000100000000` | `0x0000000000000001` | `1`           |
| 47219696000   | 46112984   | 0       | 0       | 198053758200971264  | `0x02bfa0d800000000` | `0x0000000002bfa0d8` | `37U5o`       |
| 47219696000   | 46112984   | 0       | 1       | 198053758200971265  | `0x02bfa0d800000001` | `0x0000020002bfa0d8` | `ciNixiq`     |
| 47219696000   | 46112984   | 1       | 0       | 198053758205165568  | `0x02bfa0d800400000` | `0x0000000082bfa0d8` | `2OS6gq`      |
| 47219696000   | 46112984   | 25      | 0       | 198053758305828864  | `0x02bfa0d806400000` | `0x0000000c82bfa0d8` | `weR31c`      |
| 47219696000   | 46112984   | 613     | 12345   | 198053760772091961  | `0x02bfa0d899403039` | `0x0060733282bfa0d8` | `20L2JaLj3o`  |
| 2199023254528 | 2147483647 | 1023    | 4194303 | 9223372036854775807 | `0x7fffffffffffffff` | `0x7fffffffffffffff` | `AzL8n0Y58m7` |

Implementations SHOULD verify both directions: field assembly → ID →
string, and string → ID → fields.

## 10. References

- Twitter Snowflake (reference implementation):
  <https://github.com/twitter-archive/snowflake/tree/snowflake-2010>
- RFC 2119: Key words for use in RFCs to Indicate Requirement Levels
