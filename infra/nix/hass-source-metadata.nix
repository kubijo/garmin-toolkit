{ lib, roots }:
let
  packages = lib.genericClosure {
    startSet = map (package: {
      key = package.drvPath;
      inherit package;
    }) roots;
    operator =
      { package, ... }:
      map
        (dependency: {
          key = dependency.drvPath;
          package = dependency;
        })
        (
          lib.filter lib.isDerivation (
            (package.buildInputs or [ ])
            ++ (package.propagatedBuildInputs or [ ])
            ++ (package.nativeBuildInputs or [ ])
            ++ (package.propagatedNativeBuildInputs or [ ])
            ++ (package.runtimeInputs or [ ])
            ++ lib.optionals (builtins.isList (package.pythonPath or null)) package.pythonPath
            ++ lib.optionals (package ? stdenv.cc) [ package.stdenv.cc ]
            ++ lib.optionals (package ? libc) [ package.libc ]
            ++ lib.optionals (package ? cc) [ package.cc ]
            ++ lib.optionals (package ? unwrapped) [ package.unwrapped ]
          )
        );
  };
in
map (
  { key, package }:
  {
    derivation = key;
    name = package.name or "unknown";
    outputs = map (output: (package.${output} or package).outPath) (package.outputs or [ "out" ]);
    licenses = map (license: {
      id =
        if license ? licenseType then
          lib.licenses.toSPDX license
        else
          license.spdxId or license.shortName or "NOASSERTION";
      free = if license ? licenseType then lib.licenses.isFree license else license.free or false;
    }) (lib.toList (package.meta.license or [ ]));
  }
) packages
