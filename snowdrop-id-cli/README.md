# snowdrop-id-cli

`snowdrop` — command-line tool for generating, encoding, and decoding
[Snowdrop IDs](https://github.com/arcanyx-pub/snowdrop-id-rs): 63-bit,
roughly monotonic, Snowflake-interchangeable identifiers with a very
short base62 form.

## Install

```console
$ cargo install snowdrop-id-cli
```

## Usage

```console
$ snowdrop generate -n 2
198358378861297664	37mXl
198358378861297665	ciYFJPE

$ snowdrop decode 37mXl
id:           198358378756440064
hex:          0x02c0b5e500000000
base62:       37mXl
timestamp:    46183909
machine-id:   0
sequence:     0
window-start: 2026-07-02T08:45:22.816Z (1782981922816 ms, epoch 1735689600000 ms)

$ snowdrop encode 198358378756440064
37mXl
```

Run `snowdrop --help` for all options (machine ID, count, custom epoch).

For the ID format and generation algorithm, see the
[specification](https://github.com/arcanyx-pub/snowdrop-id-rs/blob/main/SPEC.md).
The Rust library is the [`snowdrop-id`](https://crates.io/crates/snowdrop-id)
crate.

## License

MIT
