export type PluginOptions = {
  websiteID: string;
  dataHostURL?: string;
  dataAutoTrack?: boolean;
  dataDoNotTrack?: boolean;
  dataCache?: boolean;
  dataDomains?: string;
  dataExcludeSearch?: boolean;
  dataExcludeHash?: boolean;
  dataTag?: string;
  dataBeforeSend?: string;
};

export type Options = Partial<PluginOptions>;