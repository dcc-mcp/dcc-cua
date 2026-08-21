import { defineConfig } from "wxt";

export default defineConfig({
  manifest: ({ browser }) => ({
    name: "DCC-CUA Browser Provider",
    description: "Explicitly pair one browser tab with the project-owned DCC-CUA runtime",
    permissions: ["activeTab", "nativeMessaging", "scripting", "storage"],
    action: {},
    ...(browser === "firefox"
      ? {
          browser_specific_settings: {
            gecko: {
              id: "dcc-cua@dcc-mcp.org",
              strict_min_version: "140.0",
              data_collection_permissions: {
                required: ["browsingActivity", "websiteContent", "websiteActivity"],
              },
            },
          },
        }
      : {}),
  }),
  zip: {
    artifactTemplate: "dcc-cua-browser-extension-{{version}}-{{browser}}.zip",
    sourcesTemplate: "dcc-cua-browser-extension-{{version}}-sources.zip",
  },
});
