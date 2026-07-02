# snowdrop-id-rs

Rust implementation of Snowdrop ID, a smaller, cuter alternative to Snowflake.

<p align="center">
  <img src="assets/snowdrop.jpg" alt="Snowdrop, the Heeler puppy mascot, holding a snowdrop flower in a snowy forest" width="600">
</p>

A Snowdrop ID is a 63-bit, roughly monotonic, collision-free identifier —
like a Snowflake ID — that additionally encodes to a very short base62
string (7 characters or fewer in the common case, as few as 5) and
interleaves exactly with Snowflake IDs in the same BTree keyspace.

See [SPEC.md](SPEC.md) for the format, generation algorithm, encoding,
and test vectors.
