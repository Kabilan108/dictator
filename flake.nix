{
  description = "native speech to text daemon for linux";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          lib = pkgs.lib;
        in
        rec {
          default = pkgs.rustPlatform.buildRustPackage rec {
            pname = "dictator";
            version = "3.0.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
              # One entry covers every crate from the Rune repository. Bump it
              # with the `rev` in Cargo.toml: set it to lib.fakeHash, build,
              # and take the hash the error prints.
              outputHashes."rune-0.0.1" = "sha256-7XrJm3glNR9hrp4VfBL1w93lOuCk9sT5wlCIJ5wIQm0=";
            };

            DICTATOR_VERSION = version;

            postInstall = ''
              wrapProgram $out/bin/dictator --prefix PATH : ${lib.makeBinPath [ pkgs.pulseaudio ]}
              # Shell completions (clap-generated)
              install -d $out/share/bash-completion/completions
              $out/bin/dictator completion bash > $out/share/bash-completion/completions/dictator

              install -d $out/share/zsh/site-functions
              $out/bin/dictator completion zsh > $out/share/zsh/site-functions/_dictator

              install -d $out/share/fish/vendor_completions.d
              $out/bin/dictator completion fish > $out/share/fish/vendor_completions.d/dictator.fish
            '';

            # Tests that spawn /bin/sh can fork while a parallel test holds a
            # flock or listening socket; the child keeps that descriptor alive
            # until exec, so the lock and stale-socket tests fail intermittently.
            checkFlags = [ "--test-threads=1" ];

            meta = with lib; {
              description = "native speech to text daemon for linux";
              homepage = "https://github.com/kabilan108/dictator";
              license = licenses.asl20;
              platforms = [ system ];
              mainProgram = "dictator";
            };

            # nixpkgs links -sys crates against system libraries rather than
            # their bundled copies; rusqlite's libsqlite3-sys needs this one.
            buildInputs = [ pkgs.sqlite ];
            nativeBuildInputs = with pkgs; [
              pkg-config
              makeWrapper
            ];
          };
          gui = default.overrideAttrs (old: {
            pname = "dictator-gui";
            cargoBuildFeatures = [ "gui" ];
            cargoCheckFeatures = [ "gui" ];
            buildInputs =
              old.buildInputs
              ++ (with pkgs; [
                fontconfig
                libxcb
                freetype
                libxkbcommon
                wayland
                libGL
                vulkan-loader
                openssl
              ]);
            postInstall = ''
              rm -f $out/bin/dictator
              # Named for the app id the window reports, so launchers and
              # taskbars match the running window to this entry and its icon.
              install -Dm644 assets/dictator.svg $out/share/icons/hicolor/scalable/apps/org.dictator.Dictator.svg
              install -Dm644 assets/org.dictator.Dictator.desktop $out/share/applications/org.dictator.Dictator.desktop
              wrapProgram $out/bin/dictator-gui \
                --prefix PATH : ${
                  lib.makeBinPath [
                    pkgs.ffmpeg
                    pkgs.pulseaudio
                  ]
                } \
                --prefix LD_LIBRARY_PATH : ${
                  lib.makeLibraryPath (
                    with pkgs;
                    [
                      libxkbcommon
                      wayland
                      libGL
                      vulkan-loader
                      fontconfig
                      freetype
                      libx11
                      libxcb
                      libxcursor
                      libxi
                      libxrandr
                    ]
                  )
                }
            '';
            meta = old.meta // {
              description = "Dictator desktop application";
              mainProgram = "dictator-gui";
            };
          });
        }
      );
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          commonPackages = with pkgs; [
            python3
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            ffmpeg
            pulseaudio
            pkg-config
            libxcb
            fontconfig
            freetype
            libxkbcommon
            wayland
            libGL
            vulkan-loader
            openssl
          ];
          guiLibraries = with pkgs; [
            libxkbcommon
            wayland
            libGL
            vulkan-loader
            fontconfig
            freetype
            libx11
            libxcb
            libxcursor
            libxi
            libxrandr
          ];
          guiEnvironment = {
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath guiLibraries;
          };
          x11Packages = with pkgs; [
            xclip
            xdotool
          ];
          waylandPackages = with pkgs; [
            wl-clipboard
            wtype
          ];
        in
        {
          default = pkgs.mkShell (
            guiEnvironment
            // {
              buildInputs = commonPackages ++ waylandPackages;
            }
          );
          wayland = pkgs.mkShell (
            guiEnvironment
            // {
              buildInputs = commonPackages ++ waylandPackages;
            }
          );
          x11 = pkgs.mkShell (
            guiEnvironment
            // {
              buildInputs = commonPackages ++ x11Packages;
            }
          );
        }
      );
      homeManagerModules = {
        dictator = import ./nix/home-manager.nix { inherit self; };
        default = self.homeManagerModules.dictator;
      };
    };
}
