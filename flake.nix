{
  description = "A privacy-focused merged iCalendar feed server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      packageVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          craneLib = crane.mkLib pkgs;
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              pkgs.lib.cleanSourceFilter path type
              && !(builtins.elem (builtins.baseNameOf path) [
                "target"
                "result"
                ".jj"
              ]);
          };
          commonArgs = {
            pname = "ical-merger";
            version = packageVersion;
            inherit src;
            strictDeps = true;
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          app = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs = "--bin ical-merger";
            }
          );
          image = pkgs.dockerTools.buildLayeredImage {
            name = "ical-merger";
            tag = "latest";
            contents = [
              app
              pkgs.cacert
            ];
            config = {
              Entrypoint = [ "${app}/bin/ical-merger" ];
              User = "65532:65532";
              ExposedPorts = {
                "3000/tcp" = { };
              };
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=ical_merger=info"
              ];
            };
          };
        in
        {
          default = app;
          ical-merger = app;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          dockerImage = image;
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          craneLib = crane.mkLib pkgs;
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: type:
              pkgs.lib.cleanSourceFilter path type
              && !(builtins.elem (builtins.baseNameOf path) [
                "target"
                "result"
                ".jj"
              ]);
          };
          commonArgs = {
            pname = "ical-merger";
            version = packageVersion;
            inherit src;
            strictDeps = true;
            SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          app = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs = "--bin ical-merger";
            }
          );
        in
        {
          nixFormatting =
            pkgs.runCommand "ical-merger-nixfmt"
              {
                nativeBuildInputs = [ pkgs.nixfmt ];
              }
              ''
                nixfmt --check ${./flake.nix}
                touch $out
              '';
          formatting = craneLib.cargoFmt { inherit src; };
          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- -D warnings";
            }
          );
          tests = craneLib.cargoTest (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoTestExtraArgs = "--all-targets";
            }
          );
          helmChart =
            pkgs.runCommand "ical-merger-helm-chart"
              {
                nativeBuildInputs = [ pkgs.kubernetes-helm ];
              }
              ''
                helm lint ${./charts/ical-merger} --values ${./charts/ical-merger}/ci/values.yaml
                helm template availability ${./charts/ical-merger} --values ${./charts/ical-merger}/ci/values.yaml > rendered.yaml
                test -s rendered.yaml
                grep -q 'output = "title"' rendered.yaml
                grep -q 'url = "https://calendar.example.test/work.ics"' rendered.yaml
                helm template availability-secret ${./charts/ical-merger} \
                  --set config.existingConfigSecret=ical-merger-input > secret-rendered.yaml
                grep -q 'secretName: ical-merger-input' secret-rendered.yaml
                if grep -q 'kind: ConfigMap' secret-rendered.yaml; then
                  exit 1
                fi
                touch $out
              '';
          package = app;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              kubernetes-helm
              nixfmt
              rustc
              rustfmt
            ];
          };
        }
      );

      formatter = forAllSystems (system: (import nixpkgs { inherit system; }).nixfmt);
    };
}
