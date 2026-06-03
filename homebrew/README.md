# understatus Homebrew Formula

This directory contains the Homebrew formula for understatus, staged here in the main repository. The actual tap that users install from is a **separate repository**: `ictechgy/homebrew-understatus`.

---

## Directory layout

```
homebrew/
  understatus.rb   <- the formula (staged here; must be copied to the tap repo)
  README.md        <- this file
```

The tap repo expects the formula at:

```
Formula/understatus.rb
```

---

## Step-by-step: publishing a release

### 1. Push the version tag

Ensure `Cargo.toml` has `version = "0.1.0"`, then push the tag that triggers the release workflow:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The `.github/workflows/release.yml` workflow builds two release assets on GitHub Actions:

| Asset | Runner |
|---|---|
| `understatus-0.1.0-aarch64-apple-darwin.tar.gz` | `macos-14` (Apple Silicon) |
| `understatus-0.1.0-x86_64-apple-darwin.tar.gz` | `macos-13` (Intel) |

Each tarball contains a single executable named `understatus` at its root.

### 2. Obtain the SHA-256 of the source tarball

After the tag is pushed (and GitHub has processed it), download the auto-generated source tarball and hash it:

```sh
curl -L https://github.com/ictechgy/understatus/archive/refs/tags/v0.1.0.tar.gz \
  | shasum -a 256
```

This prints a line like:

```
a3f2...dead  -
```

Copy the hex digest (the part before the two spaces).

### 3. Fill in the sha256 placeholder

Open `homebrew/understatus.rb` and replace:

```ruby
sha256 "REPLACE_WITH_SOURCE_TARBALL_SHA256"
```

with the actual digest:

```ruby
sha256 "a3f2...dead"
```

### 4. Copy the formula to the tap repository

The tap repository must exist at `ictechgy/homebrew-understatus` on GitHub (create it if it does not yet exist). Copy the updated formula into it:

```sh
# From the understatus repo root
cp homebrew/understatus.rb /path/to/homebrew-understatus/Formula/understatus.rb

cd /path/to/homebrew-understatus
git add Formula/understatus.rb
git commit -m "feat: add understatus formula v0.1.0"
git push origin main
```

### 5. Verify the tap works

```sh
brew tap ictechgy/understatus
brew install ictechgy/understatus/understatus
understatus --version
```

---

## For subsequent releases (e.g. v0.2.0)

1. Bump `version` in `Cargo.toml` to `0.2.0`.
2. Update the `url` and `sha256` lines in `homebrew/understatus.rb` (new tag URL + new sha256).
3. Repeat steps 1-5 above with the new version number.

---

## Formula design notes

- **Builds from source** using `cargo install` via Homebrew's `std_cargo_args`. No prebuilt binary is bundled in this formula.
- **macOS only**: the formula declares `depends_on :macos` because understatus uses macOS-exclusive APIs (`host_processor_info`, `sysctl`, IOKit FFI).
- **Rust** is listed as a build-time dependency (`depends_on "rust" => :build`) and is not required at runtime.
- The smoke test (`brew test understatus`) runs `understatus --version` and asserts the string `understatus` appears in the output.
