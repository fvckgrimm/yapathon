# Overlay

This is the part that catches people, so it is worth being precise.

The window's Run tab has the URL in a chip at the bottom — **click it and it
copies**. Then in your scene: **Sources → + → Browser**.

| Setting | Value |
| ------- | ----- |
| URL | `http://127.0.0.1:8973/overlay` (click it in the app to copy) |
| Width | `480` (or your scene width) |
| Height | `270` (or your scene height) |

The overlay polls `/api/state` every two seconds as well as subscribing to the
WebSocket, because OBS's CEF blocks WebSockets to private-network addresses on
some configurations and a source whose only feed is the socket would then sit on
its initial state forever.

It starts visible even with no yapathon running — it says "no yapathon" rather
than showing nothing — so an empty source is always distinguishable from a broken
one.

### If the Browser Source shows nothing

Work down this list in order. Each step rules out a different cause, and the
first two are one screenshot each rather than a guess.

**1. Is the app running?**

```sh
curl -s http://127.0.0.1:8973/ping
```

That prints `ok`. If it hangs or refuses, the app is not running — start it and
watch for the two URL lines it prints.

**2. Can OBS reach the app at all?**

Put this URL in a Browser Source:

```
http://127.0.0.1:8973/ping
```

Just `ok` in tiny text, but visible. Plain text goes through no CSS engine, no
layout and no JavaScript, so if you see it, the network path is fine and the
problem is in the page. **If this shows nothing, stop reading this list** — it is
not a CSS problem:

- **Is the app still running?** Closing the dock window does *not* stop the server
  (that used to be a bug here), but quitting the app does.
- **Is the source in the scene you are streaming?** It has to be in the scene
  selected under *Scene* in the preview, and the preview has to be showing. A
  source can be perfectly correct and invisible if the scene is not active.
- **Is "Shutdown source when not visible" checked?** It is off by default. If it
  is on, OBS tears the page down whenever the source is not in the active scene,
  and a source whose scene is never active never loads at all.

**3. Does the page itself render?**

```
http://127.0.0.1:8973/overlay?debug=1
```

The whole canvas goes solid magenta with the URL, viewport size and whether the
API answered.

- **Magenta** → OBS is loading the page fine and something in the overlay's own
  styling does not render. Carry on to step 4.
- **Nothing** → OBS is not loading the page, which is step 2 again.

**4. Strip it back to what OBS definitely renders.**

```
http://127.0.0.1:8973/overlay?opaque=1
```

Same overlay, but the card's background is fully opaque instead of
`rgba(16,16,20,0.92)`. If this shows and the default does not, your GPU path is
having trouble compositing alpha, and this URL is the workaround.

**5. Check the two remaining silent causes.**

- **Width and height are zero.** A Browser Source defaults to a usable size, but
  one created from copied settings or resized to nothing has nothing to draw
  into. The card needs at least 16rem of width.
- **Stale cache.** CEF caches hard. The page is served `no-store`, but if you
  have changed the app and still see the old build, right-click the source →
  Refresh rather than adding a query string.

### A note on `backdrop-filter`

The card has no `backdrop-filter`, and that is deliberate. OBS's CEF does not
render it reliably — it can promote the element into a compositing layer that
comes back empty, taking the card with it, while the same page looks perfect in a
real browser. That is exactly the failure this overlay had: valid DOM, correct
layout, nothing on screen in OBS. The same goes for `filter`, `text-shadow` and
`mix-blend-mode`. There is a test that fails if any of them reappear.

#
