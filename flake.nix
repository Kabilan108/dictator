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
            version = "2.4.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

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

            # ensure tests run under nix
            # doCheck = true;

            meta = with lib; {
              description = "native speech to text daemon for linux";
              homepage = "https://github.com/kabilan108/dictator";
              license = licenses.asl20;
              platforms = [ system ];
              mainProgram = "dictator";
            };

            buildInputs = [ ];
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
            nativeBuildInputs =
              old.nativeBuildInputs
              ++ (with pkgs; [
                cmake
                ninja
                clang
              ]);
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            postInstall = ''
              rm -f $out/bin/dictator
              install -Dm644 assets/dictator.svg $out/share/icons/hicolor/scalable/apps/dictator.svg
              install -Dm644 assets/dictator.desktop $out/share/applications/dictator.desktop
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
              description = "Dictator GPUI desktop and tray application";
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
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            ffmpeg
            pulseaudio
            pkg-config
            fontconfig
            freetype
            libxkbcommon
            wayland
            libGL
            vulkan-loader
            openssl
            cmake
            ninja
            clang
            llvmPackages.libclang
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
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
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
