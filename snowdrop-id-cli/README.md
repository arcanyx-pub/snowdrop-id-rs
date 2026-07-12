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
69665877074640896	163eZ
69665877074640897	ciLhXHb

$ snowdrop decode 163eZ
id:           69665877074640896
hex:          0x00f780bf00000000
base62:       163eZ
timestamp:    16220351
machine-id:   0
sequence:     0
window-start: 2026-07-12T05:47:19.424Z (1783835239424 ms, epoch 1767225600000 ms)

$ snowdrop encode 69665877074640896
163eZ
```

Run `snowdrop --help` for all options (machine ID, count, custom epoch).

For the ID format and generation algorithm, see the
[specification](https://github.com/arcanyx-pub/snowdrop-id-rs/blob/main/SPEC.md).
The Rust library is the [`snowdrop-id`](https://crates.io/crates/snowdrop-id)
crate.

## License

MIT
