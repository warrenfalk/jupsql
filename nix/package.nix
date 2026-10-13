{
  lib,
  rustPlatform,
  nodejs,
  pnpm,
  fetchPnpmDeps,
  pnpmConfigHook,
  pkg-config,
  wrapGAppsHook3,
  copyDesktopItems,
  makeDesktopItem,
  desktop-file-utils,
  openssl,
  webkitgtk_4_1,
  gtk3,
  librsvg,
  glib-networking,
  mesa,
}:
let
  manifest = builtins.fromJSON (builtins.readFile ../package.json);
  dependencyFiles = lib.fileset.unions [
    ../package.json
    ../pnpm-lock.yaml
    ../pnpm-workspace.yaml
  ];
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "jupsql";
  inherit (manifest) version;

  # Include build inputs explicitly so local databases, logs, and credentials
  # never enter the package source, even when building from a path flake.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      dependencyFiles
      ../src
      ../assets
      ../index.html
      ../tsconfig.json
      ../vite.config.ts
      ../src-tauri/src
      ../src-tauri/icons
      ../src-tauri/capabilities
      ../src-tauri/Cargo.toml
      ../src-tauri/Cargo.lock
      ../src-tauri/build.rs
      ../src-tauri/tauri.conf.json
    ];
  };

  cargoRoot = "src-tauri";
  buildAndTestSubdir = finalAttrs.cargoRoot;
  cargoLock.lockFile = ../src-tauri/Cargo.lock;
  cargoBuildFeatures = [ "custom-protocol" ];
  cargoBuildFlags = [
    "--bin"
    "jupsql"
  ];
  cargoTestFlags = [ "--lib" ];

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version;
    inherit pnpm;
    src = lib.fileset.toSource {
      root = ../.;
      fileset = dependencyFiles;
    };
    fetcherVersion = 4;
    prePnpmInstall = ''
      export pnpm_config_store_dir="$storePath"
    '';
    hash = "sha256-CouCJQbhCo9dVtVBvNoR30E/JoluXeUF4/KbpKIQpt0=";
  };

  prePnpmInstall = ''
    export pnpm_config_store_dir="$STORE_PATH"
  '';
  nativeBuildInputs = [
    nodejs
    pnpm
    pnpmConfigHook
    pkg-config
    wrapGAppsHook3
    copyDesktopItems
    desktop-file-utils
  ];
  buildInputs = [
    openssl
    webkitgtk_4_1
    gtk3
    librsvg
    glib-networking
  ];

  preBuild = ''
    pnpm build
  '';
  preCheck = ''
    pnpm test
  '';

  desktopItems = [
    (makeDesktopItem {
      name = "dev.warren.jupsql";
      desktopName = "JupSQL";
      comment = "PostgreSQL notebook with inline query results";
      exec = "jupsql";
      icon = "jupsql";
      categories = [
        "Development"
        "Database"
      ];
      terminal = false;
    })
  ];

  postInstall = ''
    install -Dm644 assets/app-icon.svg "$out/share/icons/hicolor/scalable/apps/jupsql.svg"
  '';
  # Match the development shell's Mesa/WebKit runtime instead of mixing it
  # with graphics libraries from a differently pinned host system.
  preFixup = ''
    gappsWrapperArgs+=(
      --set __EGL_VENDOR_LIBRARY_FILENAMES "${mesa}/share/glvnd/egl_vendor.d/50_mesa.json"
      --set GBM_BACKENDS_PATH "${mesa}/lib/gbm"
      --set LIBGL_DRIVERS_PATH "${mesa}/lib/dri"
    )
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    test -x "$out/bin/jupsql"
    test -f "$out/share/icons/hicolor/scalable/apps/jupsql.svg"
    desktop-file-validate "$out/share/applications/dev.warren.jupsql.desktop"
    runHook postInstallCheck
  '';

  meta = {
    description = "PostgreSQL notebook with inline results and reusable queries";
    homepage = "https://github.com/warrenfalk/jupsql";
    mainProgram = "jupsql";
    platforms = lib.platforms.linux;
  };
})
