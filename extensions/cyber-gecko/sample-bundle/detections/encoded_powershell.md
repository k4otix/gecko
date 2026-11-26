---
type: concept
title: Encoded PowerShell command line
description: Flags powershell.exe started with an -EncodedCommand argument, a common way to hide a script's content (ATT&CK T1059.001).
tags: [detection, windows, powershell]
---

# Encoded PowerShell command line

Alerts when `powershell.exe` or `pwsh.exe` starts with `-EncodedCommand` (or a prefix
of it, such as `-enc`) and a base64 payload. Attackers encode a script to keep its
content out of command-line logs.

- **Technique:** T1059.001, PowerShell.
- **Reads:** process-creation events (Sysmon event 1), and PowerShell script-block
  logging (event 4104), which records the decoded script.
- **Known benign sources:** some management agents launch encoded commands; tune them
  out by parent process.
