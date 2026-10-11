# Twitch

All three of these are needed; a partial config is treated as no config rather
than half-working, because EventSub refuses the socket without a client id and
silently receives nothing without a token.

1. Register an application at [dev.twitch.tv/console/apps](https://dev.twitch.tv/console/apps).
2. Get a user access token with the channel scopes.
3. Store the token with **Settings → Twitch → Token → Set…**.
4. Set the channel login and the application id.

```toml
twitch_channel = "your_login"
twitch_client_id = "your_app_id"
```

Without any of this, chat commands still work and a yapathon is still playable —
which is also the fastest way to try it.
