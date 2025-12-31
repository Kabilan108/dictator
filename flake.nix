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
        {
          default = pkgs.buildGoModule rec {
            pname = "dictator";
            version = "2.2.0";
            src = ./.;
            vendorHash = "sha256-5x920a+jLyjndwIstLW7lGUDgF92QNe1hMMot7O9Uoc=";

            buildPhase = ''
              runHook preBuild
              make build VERSION=${version}
              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall

              install -Dm755 build/dictator $out/bin/dictator

              # Shell completions (Cobra-generated)
              install -d $out/share/bash-completion/completions
              $out/bin/dictator completion bash > $out/share/bash-completion/completions/dictator

              install -d $out/share/zsh/site-functions
              $out/bin/dictator completion zsh > $out/share/zsh/site-functions/_dictator

              install -d $out/share/fish/vendor_completions.d
              $out/bin/dictator completion fish > $out/share/fish/vendor_completions.d/dictator.fish

              runHook postInstall
            '';

            # ensure tests run under nix
            # doCheck = true;
            # checkPhase = "make test";

            meta = with lib; {
              description = "native speech to text daemon for linux";
              homepage = "https://github.com/kabilan108/dictator";
              license = licenses.asl20;
              platforms = [ system ];
              mainProgram = "dictator";
            };

            buildInputs = with pkgs; [ portaudio ];
            nativeBuildInputs = with pkgs; [ pkg-config ];
          };
        }
      );
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          commonPackages = with pkgs; [
            go
            gopls
            ffmpeg
            pkg-config
            portaudio
          ];
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
          default = pkgs.mkShell {
            buildInputs = commonPackages ++ waylandPackages;
          };
          wayland = pkgs.mkShell {
            buildInputs = commonPackages ++ waylandPackages;
          };
          x11 = pkgs.mkShell {
            buildInputs = commonPackages ++ x11Packages;
          };
        }
      );
      homeManagerModules = {
        dictator = import ./nix/home-manager.nix { inherit self; };
        default = self.homeManagerModules.dictator;
      };
    };
}
