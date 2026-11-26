---
type: concept
title: Scheduled task created for persistence
description: Flags a new scheduled task whose action runs from a user-writable path (ATT&CK T1053.005).
tags: [detection, windows, persistence]
---

# Scheduled task created for persistence

Alerts when a scheduled task is registered (Security event 4698) whose action runs a
binary or script under a user-writable directory such as `%APPDATA%` or `%TEMP%`.

- **Technique:** T1053.005, Scheduled Task.
- **Reads:** task-registration events from the Windows Security log, and process
  creation (Sysmon event 1) for the `schtasks.exe` invocation.
- **Known benign sources:** per-user updaters that install into `%LOCALAPPDATA%`.
