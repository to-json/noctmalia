{
  description = "noctmalia: a Noctalia-native front end for Thunderbird";

  # The revision noctalia-iced builds against, so both repositories share a glibc and a toolchain.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/eaad089433ca2bb662274377d33df3d0e51ef28b";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEach = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forEach (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            pkg-config
            just # the justfile, for anyone who does not have it on the host
            python3 # tools/fake-bridge.py, which the contacts tests drive
            litehtml # dev headers + static lib for litehtml-sys's build.rs, see docs/html-mail-plan.md
            gumbo # litehtml's own HTML5 parser dependency — liblitehtml.a doesn't bundle it
          ];

          # litehtml has no pkg-config file (checked: nixpkgs' derivation ships only cmake config
          # files), so litehtml-sys/build.rs reads this directly instead of shelling out to
          # pkg-config or cmake.
          LITEHTML_ROOT = pkgs.litehtml;
          GUMBO_ROOT = pkgs.gumbo;

          # iced reaches the GPU through Mesa. Binaries built here link nix's glibc and loader, so
          # they cannot load the system's /usr/lib/dri drivers — nix's Mesa has to be on the path
          # instead. Without it everything silently falls back to llvmpipe, which does not look like
          # a driver problem, it looks like the application being catastrophically slow.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              wayland
              libxkbcommon
              vulkan-loader
              libglvnd
              libgbm
              mesa
            ]
          );
          LIBGL_DRIVERS_PATH = "${pkgs.mesa}/lib/dri";
          __EGL_VENDOR_LIBRARY_DIRS = "${pkgs.mesa}/share/glvnd/egl_vendor.d";
        };
      });
    };
}
