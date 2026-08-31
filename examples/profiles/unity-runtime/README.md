# Unity runtime profile example

This package covers two supported perception paths for a packaged Windows
Unity player:

- An unmodified build uses exact-window `snapshot --pixels-only`, `zoom`, and
  `live_observation`. This remains the universal path when UI Automation and
  OCR do not expose useful semantics.
- A developer-owned or test build may embed the optional read-only companion.
  It publishes active uGUI and UI Toolkit controls, visible labels, hierarchy
  paths, interactability, and top-left render-pixel rectangles over bounded
  loopback HTTP. It has no action endpoint.

The companion is not an injector and must not be attached to a third-party
binary. Because a packaged Unity player's executable name is chosen by the
product, this repository cannot ship one built-in selector that safely matches
every game. Customize the sample profile for the build under test.

## Configure and install

1. Replace `SampleUnityPlayer.exe` and the sample title in `profile.json` with
   the exact values returned by `dcc-cua list --on-screen`.
2. For a cooperative build, copy every `.cs` file under `companion/` into the
   Unity project, add `RuntimeUiStateServer` to one persistent GameObject, and
   explicitly enable **Enable State Server** in the Inspector. The default is
   off.
3. Keep the default loopback port or update the component and `profile.json`
   together.
4. Validate and install the customized package:

   ```powershell
   cargo run -p dcc-cua-cli -- profile validate examples/profiles/unity-runtime
   cargo run -p dcc-cua-cli -- profile install examples/profiles/unity-runtime
   ```

The source separates Unity collection, state models, lifecycle orchestration,
and bounded loopback transport. Unity APIs run only on the application main
thread; the read-only TCP listener runs on a background thread. It does not use
reflection, runtime patching, or a Mono-only injection API, so the same source
can be built into Mono or IL2CPP Windows players. A prebuilt IL2CPP or Mono
binary that did not opt in remains visual-only.

## Observe safely

First bind the real target:

```powershell
dcc-cua list --app SampleUnityPlayer.exe --on-screen
dcc-cua snapshot --pid $pid --window-id $hwnd --pixels-only --output unity.png
```

Then read the optional semantics:

```powershell
dcc-cua profile-state --id unity-runtime --source unity-ui
dcc-cua profile-state --id unity-runtime --source unity-ui --watch --poll-ms 100
```

Before using a rectangle, require all of the following:

- `state.application.processId` equals the bound PID;
- `state.application.windowId` equals the bound HWND and is nonzero;
- `coordinateSpace.origin` is `top_left`;
- the reported render width/height can be mapped to the fresh snapshot's
  `coordinate_space` without assuming a desktop scale factor;
- the widget still exists in the latest monotonic `tickId`.

Use the current snapshot and the README coordinate transform when the Unity
render size differs from the PNG size. Input, authorization, post-action
verification, interruption, and the visible control banner remain owned by the
DCC-CUA Host. A successful companion read is perception evidence only.

## Data minimization

The sample publishes only active interactive controls. It never publishes an
input field's current value; for input controls it uses a placeholder or object
name. Labels and paths are bounded, widgets are capped, requests have bounded
headers, and the listener accepts only `GET /v1/ui` on `127.0.0.1`.
