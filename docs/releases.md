# Builds and releases

The release workflow runs on pushes to `master` and manual dispatch. Pull
requests run Rust CI.

1. Run version checks, release-tool tests, Rust formatting, Clippy and GUI tests.
2. Build the CLI and GUI using this repository's locked nixpkgs.
3. Check the CLI version and confirm that the GUI executable exists.
4. For a new version, build the CLI outside Nix on Ubuntu 22.04 and smoke-test
   the extracted archive in an Ubuntu container without Nix.
5. Tag the tested commit, upload the tarball and checksum into a draft release,
   then publish it. Retries resume drafts; published assets are left unchanged.

Only `master` runs can build or release. Manual dispatch runs the same flow and
publishes only when the commit has a new version.

## Bump a release

```sh
direnv exec "$PWD" python3 scripts/version.py --bump 2.5.0
```

Choose the next unused version. This updates `flake.nix`, the root package in
`Cargo.toml`, and its entry in `Cargo.lock`, leaving dependencies unchanged.
Commit these changes with the release's code. CI checks all three versions
and the evaluated Nix package version. `DICTATOR_VERSION` bakes that version
into Nix and archive builds; plain Cargo builds use the manifest version.

An existing tag on the same commit permits a publishing retry. A tag on an
unrelated commit history, or a version change back to an old tag, fails.
Tags are never moved. An incomplete release must be retried at its original
commit before advancing to another release.

The pre-existing `v2.4.0` release predates the Rust port and GUI. These workflow
changes do not replace it; a new version is needed to publish the new archive.

## Dotfiles consumption

Keep using `inputs.dictator.packages.${system}.default` and `.gui`, or the
Home Manager module. Keep Dictator's own nixpkgs input for reproducible builds.

After the Dictator workflow succeeds, update only the Dictator input in dotfiles:

```sh
nix flake update dictator
```

Apply the normal system rebuild yourself. A lock update can select a newer
commit whose workflow is still running. The workflow result shows whether that
commit passes Dictator's build and test checks.
