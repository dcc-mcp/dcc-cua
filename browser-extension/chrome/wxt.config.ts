import { defineConfig } from "wxt";

const icons = {
  16: "icons/icon-16.png",
  32: "icons/icon-32.png",
  48: "icons/icon-48.png",
  128: "icons/icon-128.png",
};

export default defineConfig({
  manifest: ({ browser }) => ({
    name: "DCC-CUA Browser Provider",
    description: "Explicitly pair one browser tab with the project-owned DCC-CUA runtime",
    permissions: ["activeTab", "nativeMessaging", "scripting", "storage"],
    icons,
    action: { default_icon: icons },
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
