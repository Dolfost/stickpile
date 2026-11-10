# stickpile: sticker and meme library. Drop into nix/hosts/aorus/services/ and
# add ./stickpile.nix to the imports in default.nix. The quadlet itself
# (deploy/quadlet/stickpile.container in the stickpile repo) goes into
# containers/systemd/ like the others.
#
# Unlike the other services, the tailnet proxy does not target a TCP port but
# the server's trusted unix socket, $STICKPILE_DIR/tailscale.sock: only there
# does stickpile trust the Tailscale-User-Login header and log tailnet users
# in without a password. A loopback TCP port cannot be trusted that way with
# rootless podman, since every local user can reach it.
#
# Two ways to get that oneshot; this file uses (A).
#
# (A) Recommended: a `unix` escape hatch in mkServe (default.nix), next to
#     tcp/scheme/ssh, so stickpile stays a one-line `dotfiles.serve` entry and
#     its unit is built by the same mkUnit as every other service. The change,
#     in mkServe:
#
#     	target = if cfg ? unix
#     		then "unix:${cfg.unix}"
#     		else if cfg ? scheme
#     		then "${cfg.scheme}://127.0.0.1:${toString cfg.port}"
#     		else toString cfg.port;
#
#     and in the option description:
#
#     	  unix   = "<path>" backend is a unix socket instead of a port
#     	                    (stickpile: identity headers are only trusted
#     	                    on its socket)
#
#     (deploy/aorus/mkServe-unix.diff is the same change as a patch.)
#
# (B) Without touching default.nix: define the oneshot here, a copy of what
#     mkUnit generates. Comment out (A) below and uncomment (B).
#
# OPEN QUESTION (docs/plan.md, question 2): whether `tailscale serve
# --service=svc:...` forwards the Tailscale-User-Login / -Name headers like a
# node-level `tailscale serve` does is not verified yet. Check once deployed:
# opening https://stickpile.<tailnet>.ts.net should log you in without a
# password (or `journalctl --user -u stickpile` shows the header login). If
# Services do not forward them, tailnet users fall back to local accounts and
# tokens; nothing else changes, and the TCP port is the simpler target then:
# 	dotfiles.serve.stickpile.port = 8080;
{ config, ... }:

let
	stickpileDir = "/storage/2.5/stickpile";
in
{
	# (A)
	dotfiles.serve.stickpile.unix = "${stickpileDir}/tailscale.sock";

	# (B)
	# systemd.services.tailscale-serve-stickpile = {
	# 	description = "tailscale serve: svc:stickpile --https=443 -> unix:${stickpileDir}/tailscale.sock";
	# 	after = [ "tailscaled.service" ];
	# 	wants = [ "tailscaled.service" ];
	# 	wantedBy = [ "multi-user.target" ];
	# 	serviceConfig = {
	# 		Type = "oneshot";
	# 		RemainAfterExit = true;
	# 		Restart = "on-failure";
	# 		RestartSec = 15;
	# 		ExecStart = "${config.services.tailscale.package}/bin/tailscale "
	# 			+ "serve --yes --service=svc:stickpile --https=443 unix:${stickpileDir}/tailscale.sock";
	# 	};
	# 	unitConfig.StartLimitIntervalSec = 0;
	# };

	# All state, config.toml and secrets/ live under $STICKPILE_DIR (layout in
	# the quadlet). Files there are owned by you (UserNS=keep-id).
	home-manager.users.${config.dotfiles.user}.xdg.configFile
		."systemd/user/stickpile.service.d/env.conf".text = ''
[Service]
Environment=STICKPILE_DIR=${stickpileDir}
'';
}
