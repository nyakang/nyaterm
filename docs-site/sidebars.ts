import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docs: [
    'intro',
    {
      type: 'category',
      label: '快速开始',
      items: [
        'getting-started/installation',
        'getting-started/quick-start',
      ],
    },
    {
      type: 'category',
      label: '使用指南',
      items: [
        'guide/session-types',
        'guide/ssh-connection',
        'guide/layout-and-workspace',
        'guide/terminal',
        'guide/file-transfer',
        'guide/tunnels-and-proxy',
        'guide/quick-commands',
        'guide/otp-and-auth',
        'guide/themes',
        'guide/translation',
        'guide/security',
        'guide/sync-and-backup',
        'guide/keyboard-shortcuts',
      ],
    },
    {
      type: 'category',
      label: '开发文档',
      items: [
        'development/architecture',
        'development/setup',
        'development/frontend',
        'development/backend',
        'development/contributing',
      ],
    },
    'faq',
  ],
};

export default sidebars;
