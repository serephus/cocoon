{
  description = "Cocoon - a self-hostable time-locked pastebin";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nixit = {
      url = "github:serephus/nixit";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.naersk.follows = "naersk";
      inputs.flake-utils.follows = "flake-utils";
      inputs.rust-overlay.follows = "rust-overlay";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      nixit,
      rust-overlay,
      naersk,
      ...
    }:
    {
      # Repository settings, applied with `nix run github:serephus/nixit`.
      githubRepositories.cocoon = nixit.lib.githubRepository {
        owner = "serephus";
        name = "cocoon";

        description = "A self-hostable time-locked pastebin: submit text with a UTC timestamp and it stays private until then, then becomes public forever.";
        homepage = "https://github.com/serephus/cocoon";
        topics = [
          "rust"
          "pastebin"
          "time-lock"
          "self-hosted"
          "axum"
          "sqlite"
          "hmac"
        ];
        visibility = "public";

        features = {
          wiki.enable = false;
          issues.enable = true;
          projects.enable = false;
          discussions.enable = false;
        };

        is_template = false;
        is_archived = false;
        allow_forking = true;

        pull = {
          merge.enable = true;
          squash.enable = false;
          rebase.enable = false;
          auto_merge = true;
          delete_branch_on_merge = true;
          update_branch = true;
        };

        actions = {
          enable = true;
          policy = "all";
          default_token_permissions = "read";
          allow_pr_approval = false;
        };

        rulesets = {
          default = {
            enforcement = "active";
            conditions.ref_name.include = [ "~DEFAULT_BRANCH" ];
            rules = [
              { type = "deletion"; }
              { type = "non_fast_forward"; }
            ];
          };
          pr = {
            enforcement = "active";
            conditions.ref_name.include = [ "~DEFAULT_BRANCH" ];
            rules = [
              {
                type = "required_status_checks";
                parameters = {
                  strict_required_status_checks_policy = true;
                  required_status_checks = [
                    { "context" = "ubuntu-latest-x86_64-unknown-linux-gnu-nightly"; }
                    { "context" = "ubuntu-latest-x86_64-unknown-linux-gnu-stable"; }
                  ];
                };
              }
            ];
          };
        };
      };

      # NixOS module. The package defaults to this flake's build for the
      # host system, but can be overridden via services.cocoon-paste.package.
      nixosModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./nix/module.nix ];
          services.cocoon-paste.package =
            lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        };
    }
    // flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rustfmt"
            "clippy"
            "rust-analyzer"
          ];
        };
        naersk' = pkgs.callPackage naersk { };
      in
      {
        packages.default = naersk'.buildPackage {
          src = ./.;
          meta = {
            description = "A self-hostable time-locked pastebin";
            homepage = "https://github.com/serephus/cocoon";
            # GLWT is not in nixpkgs, so declare it inline.
            license = {
              shortName = "GLWT";
              fullName = "GLWT(Good Luck With That) Public License";
              free = true;
              url = "https://github.com/serephus/cocoon/blob/main/LICENSE";
            };
            mainProgram = "cocoon";
            maintainers = [
              {
                name = "serephus";
                email = "i@sereph.us";
                github = "serephus";
              }
            ];
          };
        };
        devShells.default =
          with pkgs;
          mkShell {
            name = "cocoon";
            buildInputs = [ rust ];
          };
      }
    );
}
