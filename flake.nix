{
  description = "JupSQL — a PostgreSQL notebook for the desktop";
  inputs.nixpkgs.url = "git+https://github.com/NixOS/nixpkgs?ref=nixos-unstable&shallow=1";

  outputs = { nixpkgs, ... }:
    let
      forSystems = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
    in {
      devShells = forSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          manifest = builtins.fromJSON (builtins.readFile ./package.json);
        in {
          default = assert manifest.packageManager == "pnpm@${pkgs.pnpm.version}"; pkgs.mkShell {
            packages = with pkgs; [ nodejs_24 pnpm cargo rustc rustfmt clippy pkg-config postgresql_18 openssl ]
              ++ lib.optionals stdenv.hostPlatform.isLinux [ webkitgtk_4_1 gtk3 librsvg glib-networking xvfb-run dbus ];
            buildInputs = with pkgs; [ openssl ] ++ lib.optionals stdenv.hostPlatform.isLinux [ webkitgtk_4_1 gtk3 librsvg ];
            GIO_MODULE_DIR = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux "${pkgs.glib-networking}/lib/gio/modules";
            # Keep Mesa and WebKit on the same libc. Host GPU libraries may use a newer libc.
            __EGL_VENDOR_LIBRARY_FILENAMES = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux "${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json";
            GBM_BACKENDS_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux "${pkgs.mesa}/lib/gbm";
            LIBGL_DRIVERS_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux "${pkgs.mesa}/lib/dri";
            shellHook = ''
              export pnpm_config_store_dir="$PWD/.cache/pnpm-store"
              export npm_config_store_dir="$PWD/.cache/pnpm-store"
              export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
              export JUPSQL_DATA_DIR="$PWD/.local-data"
            '';
          };
        });
    };
}
