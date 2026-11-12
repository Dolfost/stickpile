# Example NixOS configuration for services.stickpile, evaluated by the
# `nixos-module` flake check. It sets every option (no nulls), so the server's
# real loader sees a value of every type the module can produce.
{ ... }:
{
  services.stickpile = {
    enable = true;
    openFirewall = false;
    settings = {
      server = {
        listen = "127.0.0.1:8080";
        public_url = "https://stickpile.example.ts.net";
        data_dir = "/var/lib/stickpile";
        log = "stickpile_server=debug,info";
        secure_cookies = true;
      };
      trusted_proxy = {
        # tailscale serve --service=svc:stickpile --https=443 unix:/run/stickpile/tailscale.sock
        socket = "/run/stickpile/tailscale.sock";
        socket_mode = "0660";
        secret_file = "/run/secrets/stickpile-proxy-secret";
        secret_header = "X-Stickpile-Proxy-Secret";
      };
      auth = {
        registration = "invite";
        tailscale_autocreate = true;
        tailscale_admins = [ "alice@github" ];
        session_ttl_days = 30;
        totp_issuer = "stickpile";
        initial_admin_username = "admin";
        initial_admin_password_file = "/run/secrets/stickpile-admin-password";
      };
      limits = {
        max_upload_mb = 50;
        max_request_mb = 500;
        max_pixels = 40000000;
        max_animation_frames = 3000;
        max_video_seconds = 120;
        login_per_minute = 10;
        upload_per_minute = 120;
        ai_suggest_per_minute = 120;
      };
      media = {
        ffmpeg = "ffmpeg";
        ffprobe = "ffprobe";
        ffmpeg_timeout_secs = 120;
        thumb_px = 256;
        discord_gif_max_mb = 10;
        workers = 2;
        default_recommended_px = 512;
        near_duplicate_distance = 8;
      };
      ai = {
        enabled = true;
        model_dir = "/var/lib/stickpile/models";
        text_encoder = "multilingual";
        onnxruntime_library = "/run/current-system/sw/lib/libonnxruntime.so";
        threads = 2;
        vocabulary_file = "/etc/stickpile/vocabulary.toml";
        max_keywords = 5;
        max_emojis = 3;
      };
      telegram = {
        enabled = true;
        bot_token_file = "/run/secrets/stickpile-telegram-bot-token";
        api_url = "https://api.telegram.org";
        default_owner_user_id = 123456789;
      };
      git = {
        enabled = true;
        pull_interval_minutes = 60;
        allowed_schemes = [
          "https"
          "ssh"
        ];
        ssh_key_file = "/run/secrets/stickpile-git-ssh-key";
        https_token_file = "/run/secrets/stickpile-git-token";
        https_username = "x-access-token";
        author_name = "stickpile";
        author_email = "stickpile@example.org";
        max_repo_mb = 1024;
      };
    };
  };
}
