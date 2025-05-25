{
  description = "native speech to text daemon for x11";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  outputs = { self, nixpkgs }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs { inherit system; };
  in {
    # packages.${system}.default = pkgs.stdenv.mkDerivation rec {
    #   pname = "capscreen";
    #   version = "0.1.1";
    #   src = pkgs.fetchurl {
    #     url = "https://github.com/Kabilan108/capscreen/releases/download/v${version}/capscreen-linux-amd64.tar.gz";
    #     sha256 = "sha256-dsAsWE2zIcrCeYJi8RAUwiXvzGSgtbIGqsHJJSF9NgI=";
    #   };
    #   installPhase = ''
    #     mkdir -p $out/bin
    #     cp bin/capscreen $out/bin/
    #     chmod +x $out/bin/capscreen
    #   '';
    # };
    devShells.${system}.default = pkgs.mkShell {
      buildInputs = with pkgs; [
        go
        gopls
        nodejs_20
        xorg.xrandr
        ffmpeg
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
