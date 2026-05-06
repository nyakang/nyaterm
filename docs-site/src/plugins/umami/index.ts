import fs from "node:fs/promises";
import path from "node:path";

import type {
  LoadContext,
  OptionValidationContext,
  Plugin,
} from "@docusaurus/types";
import type { Options, PluginOptions } from "./options";

import { Joi } from "@docusaurus/utils-validation";

const PUBLIC_SCRIPT_PATH = "/assets/js/script.js";

function withBaseUrl(baseUrl: string, assetPath: string) {
  const cleanBase = baseUrl.replace(/\/$/, "");
  const cleanPath = assetPath.replace(/^\//, "");

  if (!cleanBase) {
    return `/${cleanPath}`;
  }

  return `${cleanBase}/${cleanPath}`;
}

export default function pluginTracker(
  context: LoadContext,
  options: PluginOptions
): Plugin {
  const {
    websiteID,
    dataHostURL,
    dataAutoTrack,
    dataDoNotTrack,
    dataCache,
    dataDomains,
    dataExcludeSearch,
    dataExcludeHash,
    dataTag,
    dataBeforeSend,
  } = options;

  const isProd = process.env.NODE_ENV === "production";

  return {
    name: "docusaurus-plugin-tracker",

    async contentLoaded({ actions }) {
      actions.setGlobalData(options);
    },

    async postBuild({ outDir }) {
      const sourceFile = path.join(__dirname, "script.js");
      const targetFile = path.join(
        outDir,
        PUBLIC_SCRIPT_PATH.replace(/^\//, "")
      );

      await fs.mkdir(path.dirname(targetFile), { recursive: true });
      await fs.copyFile(sourceFile, targetFile);
    },

    injectHtmlTags() {
      if (!isProd) {
        return {};
      }

      return {
        headTags: [
          {
            tagName: "script",
            attributes: {
              defer: true,
              src: withBaseUrl(context.baseUrl, PUBLIC_SCRIPT_PATH),

              "data-website-id": websiteID,

              ...(dataHostURL && {
                "data-host-url": dataHostURL,
              }),

              ...(dataAutoTrack !== undefined && {
                "data-auto-track": String(dataAutoTrack),
              }),

              ...(dataDoNotTrack !== undefined && {
                "data-do-not-track": String(dataDoNotTrack),
              }),

              ...(dataCache !== undefined && {
                "data-cache": String(dataCache),
              }),

              ...(dataDomains && {
                "data-domains": dataDomains,
              }),

              ...(dataExcludeSearch !== undefined && {
                "data-exclude-search": String(dataExcludeSearch),
              }),

              ...(dataExcludeHash !== undefined && {
                "data-exclude-hash": String(dataExcludeHash),
              }),

              ...(dataTag && {
                "data-tag": dataTag,
              }),

              ...(dataBeforeSend && {
                "data-before-send": dataBeforeSend,
              }),
            },
          },
        ],
      };
    },
  };
}

const pluginOptionsSchema = Joi.object<PluginOptions>({
  websiteID: Joi.string().required(),

  dataHostURL: Joi.string(),

  dataAutoTrack: Joi.boolean().default(true),
  dataDoNotTrack: Joi.boolean().default(false),
  dataCache: Joi.boolean().default(false),
  dataDomains: Joi.string(),
  dataExcludeSearch: Joi.boolean().default(false),
  dataExcludeHash: Joi.boolean().default(false),
  dataTag: Joi.string(),
  dataBeforeSend: Joi.string(),
});

export function validateOptions({
  validate,
  options,
}: OptionValidationContext<Options, PluginOptions>): PluginOptions {
  return validate(pluginOptionsSchema, options);
}

export type { PluginOptions, Options };