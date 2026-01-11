{
  lib,
  python3Packages,
  gtk4,
  gtk4-layer-shell,
  gobject-introspection,
  wrapGAppsHook4,
}:

python3Packages.buildPythonApplication {
  pname = "dictator-overlay";
  version = "0.1.0";
  format = "pyproject";

  src = ./.;

  nativeBuildInputs = [
    gobject-introspection
    wrapGAppsHook4
  ];

  buildInputs = [
    gtk4
    gtk4-layer-shell
  ];

  propagatedBuildInputs = with python3Packages; [
    pygobject3
    hatchling
  ];

  dontWrapGApps = true;

  preFixup = ''
    makeWrapperArgs+=("''${gappsWrapperArgs[@]}")
  '';

  meta = with lib; {
    description = "GTK4 layer-shell overlay for Dictator streaming preview";
    license = licenses.mit;
    platforms = platforms.linux;
  };
}
