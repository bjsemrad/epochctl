{
  description = "epochctl, the control CLI for the Epoch desktop shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = fn: nixpkgs.lib.genAttrs systems (system: fn nixpkgs.legacyPackages.${system});

      mkPackage =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "epochctl";
          version = "0.1.0";
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              let
                name = baseNameOf path;
              in
              !(
                type == "directory"
                && builtins.elem name [
                  "target"
                  ".git"
                ]
              );
          };

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [
            pkgs.installShellFiles
            pkgs.makeWrapper
          ];

          # epochctl shells out to `qs` for shell IPC, so the wrapper puts Quickshell on its PATH
          # rather than relying on the user's session to provide it.
          postInstall = ''
            wrapProgram $out/bin/epochctl \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.quickshell ]}

            installShellCompletion --cmd epochctl \
              --bash <($out/bin/epochctl completions bash) \
              --zsh <($out/bin/epochctl completions zsh) \
              --fish <($out/bin/epochctl completions fish)
          '';

          meta = {
            description = "Control CLI for the Epoch desktop shell";
            license = pkgs.lib.licenses.mit;
            mainProgram = "epochctl";
            platforms = pkgs.lib.platforms.linux;
          };
        };
    in
    {
      packages = forAllSystems (pkgs: rec {
        epochctl = mkPackage pkgs;
        default = epochctl;
      });

      apps = forAllSystems (pkgs: rec {
        epochctl = {
          type = "app";
          program = "${mkPackage pkgs}/bin/epochctl";
        };
        default = epochctl;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            quickshell
          ];
        };
      });

      homeManagerModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.epochctl;
        in
        {
          options.programs.epochctl = {
            enable = lib.mkEnableOption "epochctl, the Epoch shell control CLI";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.epochctl;
              description = "The epochctl package to install.";
            };

            configDir = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "$HOME/.config/epochshell";
              description = ''
                Quickshell config directory epochctl talks to. Leave null to use the default
                ($XDG_CONFIG_HOME/epochshell); set it to point epochctl at a checkout you are
                developing against instead of the installed shell.
              '';
            };

            socket = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "%t/epochoxide.sock";
              description = ''
                EpochOxide socket epochctl connects to. Leave null to use
                $XDG_RUNTIME_DIR/epochoxide.sock, which is where EpochOxide puts it by default.
              '';
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];

            # Exported rather than baked into a wrapper so an interactive `epochctl --config ...`
            # can still override them.
            home.sessionVariables = lib.mkMerge [
              (lib.mkIf (cfg.configDir != null) { EPOCHSHELL_CONFIG_DIR = cfg.configDir; })
              (lib.mkIf (cfg.socket != null) { EPOCHOXIDE_SOCKET = cfg.socket; })
            ];
          };
        };
    };
}
