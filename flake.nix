{
  description = "Mise: a living cookbook & meal planner";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems
        (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        mise = pkgs.rustPlatform.buildRustPackage {
          pname = "mise-cookbook";
          version = "0.2.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # The export shells out to git; tests exercise it.
          nativeCheckInputs = [ pkgs.git ];
          meta = {
            description = "A living cookbook & meal planner";
            license = pkgs.lib.licenses.mit;
            mainProgram = "mise-server";
          };
        };
        default = mise;
      });

      nixosModules.mise = import ./nix/module.nix self;
      nixosModules.default = self.nixosModules.mise;

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc clippy rustfmt git ];
        };
      });
    };
}
