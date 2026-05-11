import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

const config: Config = {
  title: 'NyaTerm',
  tagline: 'A desktop client for SSH-centric operations and mixed terminal workflows.',
  favicon: 'img/logo.svg',

  url: 'https://nyaterm.app',
  baseUrl: '/',

  organizationName: 'CoderKang',
  projectName: 'nyaterm',

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'ignore',

  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  i18n: {
    defaultLocale: 'zh-CN',
    locales: ['zh-CN', 'en'],
    localeConfigs: {
      'zh-CN': {
        label: '简体中文',
        direction: 'ltr',
      },
      en: {
        label: 'English',
        direction: 'ltr',
      },
    },
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          editUrl: 'https://github.com/nyakang/nyaterm/edit/main/docs-site/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  plugins: [
    [
      "./src/plugins/umami/index.ts",
      {
        websiteID: "ba8f55f2-ab09-4cf8-9266-645c4304d58f",
        dataHostURL: "https://umami.coderkang.top",
      },
    ],
  ],

  themeConfig: {
    colorMode: {
      defaultMode: 'dark',
      disableSwitch: false,
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'NyaTerm',
      logo: {
        alt: 'NyaTerm Logo',
        src: 'img/logo.svg',
      },
      items: [
        {
          to: '/#features',
          position: 'left',
          label: '功能',
          className: 'navbar__center-link navbar__center-link--features',
          activeBaseRegex: 'a^',
        },
        {
          to: '/docs/',
          position: 'left',
          label: '文档',
          className: 'navbar__center-link navbar__center-link--docs',
        },
        {
          to: '/changelog',
          position: 'left',
          label: '日志',
          className: 'navbar__center-link navbar__center-link--changelog',
        },
        {
          type: 'localeDropdown',
          position: 'right',
        },
        {
          href: 'https://github.com/nyakang/nyaterm',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: '文档',
          items: [
            {
              label: '快速开始',
              to: '/docs/getting-started/installation',
            },
            {
              label: '使用指南',
              to: '/docs/guide/ssh-connection',
            },
          ],
        },
        {
          title: '开发',
          items: [
            {
              label: '架构说明',
              to: '/docs/development/architecture',
            },
            {
              label: '贡献指南',
              to: '/docs/development/contributing',
            },
          ],
        },
        {
          title: '更多',
          items: [
            {
              label: 'GitHub',
              href: 'https://github.com/nyakang/nyaterm',
            },
            {
              label: '问题反馈',
              href: 'https://github.com/nyakang/nyaterm/issues',
            },
          ],
        },
      ],
      copyright: `Copyright &copy; ${new Date().getFullYear()} CoderKang. Built with Docusaurus.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['rust', 'toml', 'bash', 'json'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
