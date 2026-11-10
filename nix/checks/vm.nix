# NixOS VM test: the module's service starts under its full hardening and
# answers /healthz on the TCP listener and on the trusted unix socket.
{ pkgs, self }:
pkgs.testers.runNixOSTest {
  name = "stickpile";

  nodes.machine =
    { ... }:
    {
      imports = [ self.nixosModules.default ];

      services.stickpile = {
        enable = true;
        settings = {
          server = {
            public_url = "http://machine:8080";
            secure_cookies = false;
          };
          trusted_proxy.socket = "/run/stickpile/tailscale.sock";
          auth = {
            initial_admin_username = "admin";
            initial_admin_password_file = "/etc/stickpile-admin-password";
          };
        };
      };

      # Root-only, like a real secret: the service reads it through
      # LoadCredential, not directly.
      environment.etc."stickpile-admin-password" = {
        text = "correct horse battery staple";
        mode = "0400";
      };

      environment.systemPackages = [ pkgs.curl ];
    };

  testScript = ''
    machine.wait_for_unit("stickpile.service")
    machine.wait_for_open_port(8080)
    machine.succeed("curl -fsS http://127.0.0.1:8080/healthz")

    # The trusted listener exists, is a socket and is not world-accessible.
    machine.wait_for_file("/run/stickpile/tailscale.sock")
    machine.succeed("test -S /run/stickpile/tailscale.sock")
    machine.succeed("stat -c %a /run/stickpile/tailscale.sock | grep -qx 660")
    machine.succeed("curl -fsS --unix-socket /run/stickpile/tailscale.sock http://stickpile/healthz")

    # State and the secret stay with the service user.
    machine.succeed("test -f /var/lib/stickpile/stickpile.db")
    machine.succeed("stat -c %U /var/lib/stickpile/stickpile.db | grep -qx stickpile")

    # Admin subcommands find /etc/stickpile/config.toml.
    machine.succeed("sudo -u stickpile stickpile-server check-config")

    print(machine.succeed("systemd-analyze security stickpile.service | tail -n 1"))
  '';
}
