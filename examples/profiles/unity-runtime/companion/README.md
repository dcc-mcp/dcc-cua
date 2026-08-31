# Unity component

Copy every `.cs` file in this directory into a developer-owned Unity project.
Add `RuntimeUiStateServer` to one persistent GameObject and explicitly enable
the state server in the Inspector. The component supports uGUI and, on Unity
2021.1 or newer, UI Toolkit controls. It is disabled by default and exposes only
a bounded read-only endpoint on IPv4 loopback.

The optional `DCC_CUA_TMP` scripting define adds TextMeshPro label discovery
when the project already depends on TextMeshPro. Input field values are never
published.
