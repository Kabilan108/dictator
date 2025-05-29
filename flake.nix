{
  description = "native speech to text daemon for x11";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  outputs = { self, nixpkgs }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs { inherit system; };
  in {
    # packages.${system}.default = pkgs.buildGoModule rec {
    #   pname = "dictator";
    #   version = "latest";
    #   src = ./.;
    #
    #   vendorHash = "sha256-NJdXa+h3VjTrpjs1B2xi9hWyCZ9YIUry3s3zUeCzpCw=";
    #
    #   buildPhase = ''
    #     runHook preBuild
    #     make build
    #     runHook postBuild
    #   '';
    #
    #   installPhase = ''
    #     runHook preInstall
    #     mkdir -p $out/bin
    #     cp build/dictator $out/bin/
    #     runHook postInstall
    #   '';
    # };
    devShells.${system}.default = pkgs.mkShell {
      buildInputs = with pkgs; [
        go
        gopls
        nodejs_20
        xorg.xrandr
        ffmpeg
        pkg-config
        portaudio
        # self.packages.${system}.default
      ];
      shellHook = ''
        export NPM_CONFIG_PREFIX="$HOME/.npm-global"
        export PATH="$HOME/.npm-global/bin:$PATH"
        if [ ! -f "$HOME/.npm-global/bin/claude" ]; then
          npm install -g @anthropic-ai/claude-code
        fi
      '';
    };
  };
}
