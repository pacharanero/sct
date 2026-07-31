# Android (Termux)

`sct` runs on Android under [Termux](https://termux.dev) on 64-bit (`aarch64`) devices. This is
not a supported platform - there is no CI for it and no Android release target - but the
published `linux-aarch64` binary works, with one significant limitation covered below.

Local work is the realistic use case: carrying a built `snomed.db` around and querying it
offline. A full RF2 build on a phone does work - see the options below - but it is slow, and
the published binary cannot download a release at all without a workaround.

## Install Termux from F-Droid or GitHub, not the Play Store

Use the [F-Droid](https://f-droid.org/en/packages/com.termux/) build or a
[GitHub release](https://github.com/termux/termux-app/releases). The Google Play build is a
separate, experimental branch maintained in a
[different repository](https://github.com/termux-play-store/), described by the Termux
maintainers as having "extensive adjustments in order to pass policy requirements there" along
with "missing functionality and bugs" - their own recommendation is F-Droid or GitHub. Running
a downloaded native binary is among the things that does not reliably work on the Play Store
build.

Do not mix sources: Termux and its plugins must all come from the same one, or Android will
refuse to install them together.

## Install sct

```bash
pkg install curl
curl -fsSL https://raw.githubusercontent.com/pacharanero/sct/main/install.sh | sh
```

This fetches the `sct-linux-aarch64` build, which is statically linked against musl.

## `sct trud` cannot resolve DNS

This is the limitation. Every `sct trud` command fails at the connectivity pre-flight:

```
Error: Cannot reach NHS TRUD (https://isd.digital.nhs.uk/…).
...
Original error: io: failed to lookup address information: Try again
```

The network is fine - `ping 8.8.8.8` succeeds, and so does `curl https://isd.digital.nhs.uk`.
Only `sct` fails, and only on name lookup.

**Why.** The `linux-aarch64` release is a static musl binary, so it carries musl's own DNS
resolver, which reads `/etc/resolv.conf`. Android has no such file: `/etc` is a symlink to the
read-only `/system/etc`, and DNS configuration lives in system properties resolved through the
`netd` daemon. Bionic's `getaddrinfo` (used by Termux's own packages, which is why `curl` and
`pkg` work) knows how to reach `netd`; musl's does not. With no nameserver it times out and
returns `EAI_AGAIN`, which surfaces as "Try again".

`ping 8.8.8.8` is not a useful test here - it takes a literal IP address, so no name lookup
happens. To confirm DNS itself is healthy, use a Termux-native binary:

```bash
curl -sI https://isd.digital.nhs.uk | head -1     # works: Bionic resolver via netd
```

Recent versions of `sct` detect the missing `/etc/resolv.conf` and say so in the error, pointing
back to this page.

### Options

**1. Build from source in Termux (confirmed working).** Compiling under Termux links against
Bionic, so the resulting binary uses Android's resolver directly and `sct trud` works,
downloads included:

```bash
pkg install rust clang
cargo install sct-rs
```

This is the heaviest option - `rusqlite` compiles bundled SQLite, and the Arrow and Parquet
crates are large - so expect a long build and substantial memory use. Confirmed working on a
OnePlus 13 (CPH2653: Snapdragon 8 Elite, 16 GB RAM), including `sct trud download`.

Performance is better than you might expect: that handset builds a full UK Monolith edition
(837,930 concepts) through NDJSON, SQLite, Parquet, transitive closure, and FST index in about
four and a half minutes, and matches a 22-core laptop on `sct fst build`. See
[Benchmarks](benchmarks.md#android-phone-oneplus-13-termux).

**2. Build the database elsewhere.** Only `sct trud` needs the network, so if you would rather
not compile on a phone, run the download and build on a laptop, copy `snomed.db` across, and
everything else works with the released binary:

```bash
# on a computer
sct trud download --edition uk_monolith --pipeline

# copy snomed.db to the device, then on the phone
sct lookup 22298006
sct ecl "<< 73211009 |Diabetes mellitus|" --db ~/snomed.db
sct tui --db ~/snomed.db
```

`lookup`, `lexical`, `ecl`, `refset`, `map`, `codelist`, `diagram`, `tui`, `sayt`, `mcp`, and
`serve` are all local-only and need no DNS. (`sct serve` binds a local port, so a phone can
host a FHIR terminology server on your own network, which is a fun if impractical trick.)

**3. Run inside a proot distribution.** A proot rootfs has a real, writable `/etc/resolv.conf`,
so even the static binary resolves normally:

```bash
pkg install proot-distro
proot-distro install debian
proot-distro login debian
# then install sct inside Debian as usual
```

Untested by us - reasoned from how proot presents its own root filesystem. If you try it, a
report either way is welcome.

!!! warning "Do not byte-patch the binary"
    A trick circulating for other static musl CLIs on Termux is to edit the `/etc/resolv.conf`
    string inside the binary to point at a writable path. It works, but it invalidates the
    release checksum, has to be redone after every upgrade, and leaves you running a binary
    that no longer matches what we published. Prefer any of the options above.

## Why there is no Android release target

Adding `aarch64-linux-android` to the release matrix would remove the DNS limitation entirely,
at the cost of an NDK toolchain in CI for the bundled SQLite build. Since `cargo install sct-rs`
works under Termux and produces exactly such a binary, that cost currently buys convenience
rather than capability, so it is not planned. A native Android app would change the calculation.
