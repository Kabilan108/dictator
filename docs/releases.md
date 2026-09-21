# Builds, cache and releases

The release workflow runs on pushes to `master` and manual dispatch. Pull
requests run Rust CI without cache write credentials.

1. Run version checks, release-tool tests, Rust formatting, Clippy and GUI tests.
2. Build the CLI and GUI using this repository's locked nixpkgs.
3. Explicitly push those two output paths and their runtime closures to the
   public `kabilan108` Cachix cache. No development shells or build closures
   are uploaded. The cache action runs with automatic uploads disabled.
4. On a fresh runner, confirm both roots exist in Cachix and install both
   packages with local and remote builds disabled. Check the CLI version.
5. For a new version, build the CLI outside Nix on Ubuntu 22.04 and smoke-test
   the extracted archive in an Ubuntu container without Nix.
6. Tag the tested commit, upload the tarball and checksum into a draft release,
   then publish it. Retries resume drafts; published assets are left unchanged.

Set the repository secret `CACHIX_AUTH_TOKEN` to a write token for `kabilan108`.
Only the explicit upload step receives it. Manual dispatch defaults to
`cache_only`, which fills and verifies the cache without publishing a release.
Only `master` runs can upload or release.

## Bump a release

```sh
direnv exec "$PWD" python3 scripts/version.py --bump 2.5.0
```

Choose the next unused version. This updates `flake.nix`, the root package in
`Cargo.toml`, and its entry in `Cargo.lock`, leaving dependencies unchanged.
Commit these changes with the release's code. CI checks all three versions
and the evaluated Nix package version. `DICTATOR_VERSION` bakes that version
into Nix and archive builds; plain Cargo builds use the manifest version.

Commits following a release may retain its version and still populate Cachix.
An existing tag on the same commit permits a publishing retry. A tag on an
unrelated commit history, or a version change back to an old tag, fails.
Tags are never moved. An incomplete release must be retried at its original
commit before advancing to another release.

The pre-existing `v2.4.0` release predates the Rust port and GUI. These workflow
changes do not replace it; a new version is needed to publish the new archive.

## Dotfiles consumption

Keep using `inputs.dictator.packages.${system}.default` and `.gui`, or the
Home Manager module. Keep Dictator's own nixpkgs input; adding a `follows`
override can change its derivations and prevent cache hits.

After a successful cache workflow, update only the Dictator input in dotfiles:

```sh
nix flake update dictator
```

Apply the normal system rebuild yourself. A lock update can select a newer
commit whose workflow is still running; wait for that exact commit's cache
verification before rebuilding if you want to avoid compiling locally.

The cache has finite storage. Evicted outputs rebuild from source; CI can
repopulate the current revision through manual dispatch. Runtime closures
include native GUI libraries but exclude the Rust compiler and build inputs
unless those are also actual runtime references.
