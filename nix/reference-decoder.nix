{
  lib,
  clangStdenv,
  src,
}:

clangStdenv.mkDerivation {
  pname = "h4m-reference";
  version = "02d66526";
  inherit src;

  dontConfigure = true;
  buildPhase = ''
    runHook preBuild
    "$CC" -O2 -fwrapv -ffp-contract=off -DNATIVE=1 \
      h4m_audio_decode.c -o h4m-original
    "$CC" -O2 -fwrapv -ffp-contract=off \
      "-DH4M_REFERENCE_SOURCE=\"$PWD/h4m_audio_decode.c\"" \
      ${./reference-planes.c} -o h4m-planes
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    install -Dm755 h4m-original "$out/bin/h4m-original"
    install -Dm755 h4m-planes "$out/bin/h4m-planes"
    runHook postInstall
  '';

  meta = {
    description = "Pinned upstream H4M decoder and raw-plane test adapter";
    homepage = "https://github.com/mbcgh/h4m-video-decoder";
    license = lib.licenses.lgpl2Plus;
    mainProgram = "h4m-original";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
  };
}
