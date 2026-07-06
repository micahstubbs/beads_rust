# Cross-Compiling br for macOS from Linux: Why It Doesn't Work (2026-07-06)

**Goal attempted:** build both release binaries on corsair-7000d (x86_64 Linux) —
native Linux plus `aarch64-apple-darwin` — so one machine produces artifacts
for the whole fleet.

**Result: blocked.** The Linux binary builds fine. The macOS cross-build fails
and cannot be fixed without either Apple SDK headers on Linux or a change to
br's crypto stack.

## What was tried

- zig 0.16.0 + cargo-zigbuild 0.23.0 (the standard no-SDK cross toolchain)
- `rustup target add aarch64-apple-darwin` on the pinned
  `nightly-2026-02-19` toolchain
- `cargo zigbuild --release --locked --target aarch64-apple-darwin`

## Where it fails

```
aws-lc-sys v0.40.0 (build script, C compilation)
third_party/jitterentropy/.../jitterentropy-base-user.h:90:10:
fatal error: 'CoreServices/CoreServices.h' file not found
```

Dependency chain pulling it in:

```
beads_rust → self_update (br upgrade) → reqwest → rustls → aws-lc-rs → aws-lc-sys
```

## Why this is a hard blocker

- zig ships macOS **libSystem/libc** headers and Mach-O linking support, which
  covers pure-Rust and plain-C crates. It does **not** ship Apple **framework
  headers** (`CoreServices`, `CoreFoundation`, …) — those exist only in the
  macOS SDK.
- Apple's Xcode/SDK license permits using the SDK only on Apple-branded
  hardware, so copying it to corsair (the osxcross approach) is not a
  compliant option.
- `aws-lc-sys`'s bundled jitterentropy unconditionally includes
  `<CoreServices/CoreServices.h>` when targeting darwin, so the build-script C
  compile fails before linking is even attempted.

## Viable alternatives (in preference order)

1. **GitHub Actions macOS runners** — `.github/workflows/release.yml` already
   builds tag-triggered releases; macOS runners are Apple hardware, so the SDK
   is licensed and present. Blocker: this fork's Actions have never been
   enabled (0 runs all-time) — enable workflows once in the repo's Actions tab
   and tag pushes (e.g. `v0.2.3`, already pushed) will produce both platforms'
   binaries. This is the real "build once for the fleet" answer.
2. **Native build per platform** — what is deployed today: br 0.2.3 built
   natively on corsair (Linux) and on the MBA 2021 (Apple Silicon;
   requires the pinned nightly via rustup and corsair's `Cargo.lock`, since
   the lockfile is gitignored and unlocked resolution pulls a `sysinfo`
   needing a newer nightly).
3. **Swap the crypto provider** — moving reqwest/rustls off `aws-lc-rs` onto
   the `ring` provider would remove `aws-lc-sys` and likely make zigbuild
   succeed, but it changes the TLS crypto implementation shipped in releases
   for the sake of a build-host convenience. Not recommended without its own
   review; noting for completeness.

## Cleanup note

The failed cross-target artifacts live in
`~/wk/beads_rust/target/aarch64-apple-darwin/` on corsair (~1 GB); safe to
delete manually if the space matters.
