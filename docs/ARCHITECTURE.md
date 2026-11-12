# Architecture

Islora has two crates, with dependencies pointing in one direction:

```text
island-app (Windows, rendering, input, settings UI)
    └── island-core (state, rules, data, geometry)
```

`island-core` owns activity ordering, timers, notification policy, settings
values, and the data needed to describe an island frame. It has no dependency
on Windows APIs, windows, drawing resources, or the settings window. Time and
external events are passed into the core so its decisions can be tested without
an application window. Shared geometry used in hit testing lives in
`island-core::geometry`.

`island-app` turns Windows events into core inputs and renders the resulting
frames. Its `sources` modules talk to Windows and other external services;
`platform` owns windows and operating-system integration; `render` draws the
island. `settings_view` defines the settings window's controls, labels and
layout, while `platform::settings_window` draws and handles that window.

Place new behavior in the core when it can be expressed as data and decisions
without operating-system calls. Keep platform APIs, painting, and UI controls in
the app. The app can use the core; the core must not import the app.

Run `cargo test -p island-core` to check the independent rules and
`cargo test --workspace` to also check the app's view layouts and adapters.
