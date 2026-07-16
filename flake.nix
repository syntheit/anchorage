{
  description = "Anchorage — a native GTK4/libadwaita client for Linkding";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Pinned stable Rust toolchain via fenix (reproducible, works on aarch64 too).
        rustToolchain = fenix.packages.${system}.stable.toolchain;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Native build inputs needed at compile time.
        nativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook4
          glib # provides glib-compile-schemas / glib-compile-resources
          desktop-file-utils
          appstream # validate metainfo
          blueprint-compiler
        ];

        # Libraries the app links against.
        buildInputs = with pkgs; [
          glib
          gtk4
          libadwaita
          # gtk4-rs -sys crates link these directly, so they must be present at
          # link time AND on the runtime library path (see shellHook).
          pango
          cairo
          gdk-pixbuf
          graphene
          harfbuzz
          openssl
          # Secret Service backend deps are provided at runtime by the DE;
          # oo7 talks to it over D-Bus, no extra link-time libs required.
        ];

        # Cleaned source (Rust/TOML only) for the dependency layer — keeps the
        # crane cache warm across data/README/etc. edits.
        cleanSrc = craneLib.cleanCargoSource ./.;

        # Full source for the final build so postInstall can reach data/*.
        # We keep Cargo sources plus the data directory (desktop/metainfo/gschema).
        fullSrc = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./.)
            ./data
          ];
        };

        commonArgs = {
          inherit nativeBuildInputs buildInputs;
          strictDeps = true;
        };

        # Dependencies compiled against the cleaned source.
        cargoArtifacts = craneLib.buildDepsOnly (commonArgs // { src = cleanSrc; });

        anchorage = craneLib.buildPackage (
          commonArgs
          // {
            src = fullSrc;
            inherit cargoArtifacts;

            # Install the desktop file, icon, metainfo and gschema, then compile
            # the schema so the installed app launches without GSETTINGS_SCHEMA_DIR.
            postInstall = ''
              install -Dm644 data/io.matv.Anchorage.desktop \
                $out/share/applications/io.matv.Anchorage.desktop
              install -Dm644 data/io.matv.Anchorage.metainfo.xml \
                $out/share/metainfo/io.matv.Anchorage.metainfo.xml
              install -Dm644 data/icons/hicolor/scalable/apps/io.matv.Anchorage.svg \
                $out/share/icons/hicolor/scalable/apps/io.matv.Anchorage.svg
              install -Dm644 data/io.matv.Anchorage.gschema.xml \
                $out/share/glib-2.0/schemas/io.matv.Anchorage.gschema.xml
              glib-compile-schemas $out/share/glib-2.0/schemas
            '';

            # crane doesn't stamp the GUI libraries into the binary's RPATH, so the
            # wrapped app can't find libadwaita/gtk4/glib at runtime outside a full
            # GNOME session. Put them on the wrapper's LD_LIBRARY_PATH.
            preFixup = ''
              gappsWrapperArgs+=(
                --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath buildInputs}"
              )
            '';

            meta = with pkgs.lib; {
              description = "Native GTK4/libadwaita client for Linkding bookmarks";
              homepage = "https://github.com/syntheit/anchorage";
              license = licenses.gpl3Plus;
              mainProgram = "anchorage";
              platforms = platforms.linux;
            };
          }
        );
      in
      {
        packages = {
          default = anchorage;
          anchorage = anchorage;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = anchorage;
          name = "anchorage";
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ [
            rustToolchain
            fenix.packages.${system}.stable.rust-analyzer
            pkgs.clippy
          ];

          # Point gio::Settings at the locally compiled schema during dev.
          # The shellHook below compiles the schema and exports GSETTINGS_SCHEMA_DIR.
          shellHook = ''
            export GSETTINGS_SCHEMA_DIR="$PWD/data"
            # `cargo run` launches the unwrapped binary; nix build's wrapGAppsHook4
            # handles this for the packaged app, but the devshell needs the GUI
            # libs on the runtime linker path explicitly.
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath buildInputs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            if [ -f data/io.matv.Anchorage.gschema.xml ]; then
              glib-compile-schemas data 2>/dev/null || true
            fi
            echo "anchorage devshell — run: cargo run"
          '';
        };
      }
    );
}
