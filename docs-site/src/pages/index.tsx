import clsx from 'clsx';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import useBaseUrl from '@docusaurus/useBaseUrl';
import Layout from '@theme/Layout';
import Heading from '@theme/Heading';
import Link from '@docusaurus/Link';
import Translate, {translate} from '@docusaurus/Translate';

import styles from './index.module.css';

const features = [
  {
    title: <Translate>安全高效的 SSH 连接</Translate>,
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className={styles.featureSvg}>
        <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
        <path d="M7 11V7a5 5 0 0 1 10 0v4" />
        <circle cx="12" cy="16" r="1" />
      </svg>
    ),
    description: (
      <Translate>
        基于 Rust russh 库构建，提供安全、高性能的 SSH 连接。支持密码和密钥认证、代理转发、TOFU 主机密钥验证。
      </Translate>
    ),
  },
  {
    title: <Translate>集成 SFTP 文件管理</Translate>,
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className={styles.featureSvg}>
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
        <line x1="12" y1="11" x2="12" y2="17" />
        <polyline points="9 14 12 11 15 14" />
      </svg>
    ),
    description: (
      <Translate>
        内置文件浏览器，直接在侧边栏管理远程文件。支持上传、下载、重命名、权限修改等操作，传输进度实时可见。
      </Translate>
    ),
  },
  {
    title: <Translate>多标签终端界面</Translate>,
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className={styles.featureSvg}>
        <polyline points="4 17 10 11 4 5" />
        <line x1="12" y1="19" x2="20" y2="19" />
      </svg>
    ),
    description: (
      <Translate>
        同时管理多个 SSH 和本地终端会话。支持 WebGL 硬件加速渲染、自定义字体、关键词高亮、命令历史模糊搜索。
      </Translate>
    ),
  },
  {
    title: <Translate>加密同步与备份</Translate>,
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className={styles.featureSvg}>
        <path d="M12 3v12" />
        <path d="m7 10 5 5 5-5" />
        <path d="M5 19h14" />
      </svg>
    ),
    description: (
      <Translate>
        通过 WebDAV 或 S3 兼容存储同步 Dragonfly 的可移植配置，并为连接、凭据和常用设置创建可恢复的加密备份快照。
      </Translate>
    ),
  },
  {
    title: <Translate>高度可定制</Translate>,
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" className={styles.featureSvg}>
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    ),
    description: (
      <Translate>
        深色/浅色主题切换、可调整面板布局、快捷命令管理、多语言翻译、快捷键自定义，打造专属工作流。
      </Translate>
    ),
  }
];

const techStack = [
  { name: 'Tauri 2', color: '#FFC131' },
  { name: 'React 19', color: '#61DAFB' },
  { name: 'Rust', color: '#DEA584' },
  { name: 'TypeScript', color: '#3178C6' },
  { name: 'xterm.js', color: '#0EAD69' },
];

function HomepageHeader() {
  const {siteConfig} = useDocusaurusContext();
  const logoUrl = useBaseUrl('/img/logo.svg');
  return (
    <header className={styles.heroBanner}>
      <div className={styles.heroInner}>
        <div className={styles.heroLogoWrapper}>
          <img
            src={logoUrl}
            alt="Dragonfly Logo"
            className={styles.heroLogo}
          />
        </div>
        <Heading as="h1" className={styles.heroTitle}>
          {siteConfig.title}
        </Heading>
        <p className={styles.heroSubtitle}>
          <Translate>现代高性能 SSH 客户端</Translate>
        </p>
        <p className={styles.heroDescription}>
          <Translate>
            基于 Tauri 和 React 构建，跨平台、安全、快速
          </Translate>
        </p>
        <div className={styles.heroButtons}>
          <Link
            className={clsx('button button--lg', styles.btnPrimary)}
            to="/docs/getting-started/installation">
            <Translate>快速开始</Translate>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={styles.btnIcon}>
              <line x1="5" y1="12" x2="19" y2="12" />
              <polyline points="12 5 19 12 12 19" />
            </svg>
          </Link>
          <Link
            className={clsx('button button--lg', styles.btnSecondary)}
            href="https://git.coderkang.top/Tauri/dragonfly">
            <svg viewBox="0 0 24 24" fill="currentColor" className={styles.btnGithubIcon}>
              <path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0 0 24 12c0-6.63-5.37-12-12-12z" />
            </svg>
            GitHub
          </Link>
        </div>
        <div className={styles.techBadges}>
          {techStack.map((tech) => (
            <span key={tech.name} className={styles.techBadge}>
              <span className={styles.techDot} style={{backgroundColor: tech.color}} />
              {tech.name}
            </span>
          ))}
        </div>
      </div>
      <div className={styles.heroGlow} />
    </header>
  );
}

function Feature({title, icon, description}: {title: React.ReactNode; icon: React.ReactNode; description: React.ReactNode}) {
  return (
    <div className={styles.featureCard}>
      <div className={styles.featureIconWrapper}>
        {icon}
      </div>
      <Heading as="h3" className={styles.featureTitle}>{title}</Heading>
      <p className={styles.featureDescription}>{description}</p>
    </div>
  );
}

function HomepageFeatures() {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className={styles.featureGrid}>
          {features.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}


export default function Home(): React.ReactElement {
  return (
    <Layout
      title={translate({message: '首页'})}
      description={translate({message: '现代高性能 SSH 客户端，基于 Tauri 和 React 构建'})}>
      <HomepageHeader />
      <main>
        <HomepageFeatures />
      </main>
    </Layout>
  );
}
