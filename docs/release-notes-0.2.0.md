# QuotaGlass 0.2.0 release notes

QuotaGlass 0.2.0 redesigns the desktop widget into a compact pill and expandable panel, adds pace strips, and introduces limit warnings.

## Breaking changes and migrations

- Always on top is now on by default.
- v0.1.0 per-account settings were reset, so accounts must be reconnected (window position, refresh interval and autostart are kept).
- OMP support was removed.
- Orca is optional.

## What is new

### Pill and panel

QuotaGlass now runs as a compact pill docked above the taskbar. Clicking the chevron expands the pill into a two-pane panel. Pressing `Esc` or clicking the chevron closes the panel back to the pill.

### Resizable window

You can drag the pill anywhere on your screen. The pill can be stretched sideways (320 to 960 CSS pixels). The panel is resizable (minimum 440 by 344 CSS pixels). QuotaGlass remembers both sizes and restores your chosen position. Window sizes follow Windows text size.

### Pace strips and fast burn

Each limit displays as a pace strip with tick marks at 10% intervals.
- The solid fill shows the percentage of quota used.
- A marker shows the share of the time window that has passed.
- When usage runs ahead of time (`used >= 1.5 x passed` and `used >= 20`), the strip turns red and displays "fast burn".

### Best now

The account with the most room left across its limits receives a "Best now" chip. When accounts tie on room left, the account whose worst limit resets sooner wins. The mark appears when at least two accounts are readable.

### Limit warnings

Native Windows notifications alert you when any limit reaches 50, 80, or 90 percent.
- Each warning level fires once per limit window.
- Fired levels are recorded in `%APPDATA%\QuotaGlass\warnings.json`.
- The 50 and 80 percent warnings are silent toasts.
- The 90 percent warning plays a sound. It is a normal toast because the notification plugin cannot keep it on screen.

### Added accounts with profiles

You can now add extra Claude and Codex accounts beyond your primary login.
- "+ Add account" creates a profile folder in `%APPDATA%\QuotaGlass\profiles\<id>`.
- QuotaGlass opens the vendor CLI sign-in in a visible console.
- Removing an added account signs that profile out on this PC and deletes its profile folder.
- The current CLI login cannot be deleted; it can only be hidden.

### Antigravity integration

QuotaGlass reads Antigravity quota from the running Antigravity app's local server on this PC (`127.0.0.1`). Limits are read only while the app is open. QuotaGlass uses no Google sign-in.

### Theme and glass settings

Settings now include options for appearance:
- Theme: System, Light, Dark.
- Glass: Acrylic (default), Mica, Clear.
- Glass shows as a solid colour while Windows "Transparency effects" is off (Settings > Personalization > Colors). Mica looks nearly solid by design.
- Always on top toggle.
- Start with Windows toggle.
- Refresh every 1, 2 or 5 min.
- Sources: toggle individual connections on or off.
- Account names: set custom labels up to 40 characters.
- Hidden accounts: view and unhide accounts.
