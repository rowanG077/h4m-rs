{
  description = "Pure Rust H4M decoder and development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    h4m-reference = {
      url = "github:mbcgh/h4m-video-decoder/02d66526e62e346c3ef7f92eac49cc07356a2c62";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, fenix, h4m-reference }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      toolchains = forAllSystems (system:
        let fenixpkgs = fenix.packages.${system};
        in fenixpkgs.stable.withComponents [
          "cargo" "rustc" "rust-src" "rustfmt" "clippy" "rust-analyzer"
        ]);
      source = builtins.path {
        path = ./.;
        name = "h4m-source";
        filter = path: type:
          let name = baseNameOf path;
          in !(builtins.elem name [ "target" "reference" ".git" ".agents" ".codex" "result" ])
            && !(nixpkgs.lib.hasPrefix "result-" name);
      };
    in {
      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchains.${system};
            rustc = toolchains.${system};
          };
        in {
          reference-decoder = pkgs.callPackage ./nix/reference-decoder.nix {
            src = h4m-reference;
          };
          default = rustPlatform.buildRustPackage {
            pname = manifest.package.name;
            version = manifest.package.version;
            src = source;
            cargoLock.lockFile = ./Cargo.lock;
            meta = {
              description = "Pure Rust HVQM4 1.3/1.5 video decoder";
              license = pkgs.lib.licenses.lgpl2Plus;
              mainProgram = "h4m";
              platforms = systems;
            };
          };
        });
      checks = forAllSystems (system:
        let packages = self.packages.${system};
        in {
          package = packages.default;
          portable = packages.default.overrideAttrs {
            name = "h4m-portable";
            checkPhase = ''
              runHook preCheck
              cargo test --locked --offline --no-default-features
              cargo test --locked --offline --no-default-features --features alloc
              runHook postCheck
            '';
            installPhase = ''
              touch "$out"
            '';
          };
          equivalence = packages.default.overrideAttrs {
            name = "h4m-reference-equivalence";
            H4M_REFERENCE = "${packages.reference-decoder}/bin/h4m-original";
            H4M_REFERENCE_PLANES = "${packages.reference-decoder}/bin/h4m-planes";
            checkPhase = ''
              runHook preCheck
              cargo test --locked --offline --test equivalence -- --ignored --nocapture
              cargo test --locked --offline --release --test equivalence -- --ignored --nocapture
              runHook postCheck
            '';
            installPhase = ''
              touch "$out"
            '';
          };
        });
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          reference = self.packages.${system}.reference-decoder;
          releaseTools = with pkgs; [ git gh jq curl ];
        in {
          default = pkgs.mkShell {
            packages = [ toolchains.${system} reference ] ++ releaseTools
              ++ (with pkgs; [ clang actionlint shellcheck ]);
            RUST_SRC_PATH = "${toolchains.${system}}/lib/rustlib/src/rust/library";
            H4M_REFERENCE = "${reference}/bin/h4m-original";
            H4M_REFERENCE_PLANES = "${reference}/bin/h4m-planes";
          };
          release = pkgs.mkShellNoCC {
            packages = releaseTools;
          };
        });
    };
}
