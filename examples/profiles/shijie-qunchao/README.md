# 蚀界群潮 Profile

This is a declarative example profile for the internal packaged game
`DccMcpGame.exe`. It is intended for agents using dcc-cua against a real
window, not as a launcher or a game-specific automation script.

Validate it before use:

```powershell
dcc-cua profile --profile-file .\examples\profiles\shijie-qunchao\profile.json
dcc-cua profile match --profile-file .\examples\profiles\shijie-qunchao\profile.json --app DccMcpGame.exe --title 蚀界群潮
```

The profile declares the verified title-screen, combat, upgrade, victory, and
defeat vocabulary. Movement remains a fresh-observation decision: the agent
must detect the player, threats, and obstacles, choose a clearance vector, and
then send a bounded held W/A/S/D action. It must not use random movement or a
fixed patrol. `Esc` remains the Host stop control and is intentionally not a
profile pause binding; use `P` for the game's pause action.
