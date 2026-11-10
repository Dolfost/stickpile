# OCI image of the server: `nix build .#container && podman load < result`.
#
# - runs as 65532:65532, never root; no shell, no package manager
# - works with a read-only root filesystem (`podman run --read-only`,
#   quadlet `ReadOnly=true`): every write goes to the /var/lib/stickpile
#   volume, TMPDIR included
# - ffmpeg-headless and onnxruntime are in the closure through the server's
#   wrapper; CA certificates and tzdata for Telegram, git over https and TZ=
{
  pkgs,
  stickpile-server,
  version,
}:
let
  inherit (pkgs) lib;
  uid = 65532;
  dataDir = "/var/lib/stickpile";

  # The baked-in default config: only what differs from the server's defaults
  # inside a container. Mount your own over /etc/stickpile/config.toml
  # (deploy/quadlet/config.toml is a starting point).
  defaultConfig = (pkgs.formats.toml { }).generate "stickpile-container-config.toml" {
    server = {
      # Inside the container's network namespace; publish the port on the
      # host's loopback (`-p 127.0.0.1:8080:8080`).
      listen = "0.0.0.0:8080";
      data_dir = dataDir;
    };
    # Appears on the host as <volume>/tailscale.sock, for
    # `tailscale serve unix:<volume>/tailscale.sock`.
    trusted_proxy.socket = "${dataDir}/tailscale.sock";
    ai.model_dir = "${dataDir}/models";
  };

  rootfs = pkgs.runCommand "stickpile-container-rootfs" { } ''
    mkdir -p $out/etc/stickpile
    cp ${defaultConfig} $out/etc/stickpile/config.toml
    # Some libraries (libgit2's ssh transport, glibc's getpwuid) want the
    # running uid to resolve.
    cat > $out/etc/passwd <<EOF
    root:x:0:0:root:/var/empty:/sbin/nologin
    stickpile:x:${toString uid}:${toString uid}:stickpile:${dataDir}:/sbin/nologin
    nobody:x:65534:65534:nobody:/var/empty:/sbin/nologin
    EOF
    cat > $out/etc/group <<EOF
    root:x:0:
    stickpile:x:${toString uid}:
    nogroup:x:65534:
    EOF
    echo 'hosts: files dns' > $out/etc/nsswitch.conf
  '';
in
pkgs.dockerTools.buildLayeredImage {
  name = "ghcr.io/dolfost/stickpile";
  tag = version;

  # /bin/stickpile-server for `podman exec stickpile stickpile-server …`.
  contents = [
    rootfs
    stickpile-server
    pkgs.cacert
    pkgs.tzdata
  ];

  # Mount points owned by the service uid. podman creates /tmp and /run as
  # tmpfs for --read-only containers by default; nothing here relies on them.
  fakeRootCommands = ''
    mkdir -p .${dataDir} tmp
    chown ${toString uid}:${toString uid} .${dataDir}
    chmod 0750 .${dataDir}
    chmod 1777 tmp
  '';

  passthru = { inherit defaultConfig; };

  config = {
    User = "${toString uid}:${toString uid}";
    WorkingDir = dataDir;
    Entrypoint = [ (lib.getExe stickpile-server) ];
    Cmd = [ "serve" ];
    Env = [
      "STICKPILE_CONFIG=/etc/stickpile/config.toml"
      # The server creates <data_dir>/tmp at startup; temp files must live on
      # the same filesystem as media/ so finished uploads are renamed, not copied.
      "TMPDIR=${dataDir}/tmp"
      "HOME=${dataDir}"
      "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
      "TZDIR=${pkgs.tzdata}/share/zoneinfo"
      "PATH=/bin"
    ];
    ExposedPorts."8080/tcp" = { };
    Volumes.${dataDir} = { };
    Labels = {
      "org.opencontainers.image.title" = "stickpile";
      "org.opencontainers.image.description" = "Self-hosted sticker and meme library";
      "org.opencontainers.image.source" = "https://github.com/Dolfost/stickpile";
      "org.opencontainers.image.licenses" = "AGPL-3.0-or-later";
      "org.opencontainers.image.version" = version;
    };
  };
}
