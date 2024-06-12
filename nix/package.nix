{ lib
, rustPlatform
}:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version;

  src = builtins.path {
    path = ../.;
  };

  buildType = "debug";

  cargoLock.lockFile = ../Cargo.lock;

  buildInputs = [ ];

  nativeBuildInputs = [ ];

  meta = {
    description = "A CLI to live serve documentation for your crate while developing.";
    homepage = "https://github.com/ModProg/cargo-watchdoc";
    license = lib.licenses.mit;
    mainProgram = "bs";
    platforms = [ "x86_64-linux" ];
  };
}
