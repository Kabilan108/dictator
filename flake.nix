{
  description = "native speech to text daemon for x11";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  outputs = { self, nixpkgs }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs { inherit system; };
  in {
    packages.${system}.default = pkgs.buildGoModule rec {
      pname = "dictator";
      version = "latest";
      src = ./.;

      vendorHash = "sha256-zov48TzHqfWahe4HkekCtqFlgK+jxTwKzG+/h1UbVaI=";

      nativeBuildInputs = with pkgs; [ pkg-config ];
      buildInputs = with pkgs; [ portaudio ];

      buildPhase = ''
        runHook preBuild
        make build VERSION=${version}
        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall
        mkdir -p $out/bin
        cp build/dictator $out/bin/
        runHook postInstall
      '';
    };
    devShells.${system}.default = pkgs.mkShell {
      buildInputs = with pkgs; [
        go
        gopls
        xorg.xrandr
        ffmpeg
        pkg-config
        portaudio
        self.packages.${system}.default
      ];
    };
  };
}
