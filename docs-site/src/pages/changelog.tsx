import React from 'react';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import Heading from '@theme/Heading';
import Translate, {translate} from '@docusaurus/Translate';

import styles from './changelog.module.css';
import {getChangelogReleases} from '../data/changelogData';

function formatVersionLabel(version: string) {
  const match = version.match(/^\[(.+?)\]\s*-\s*(.+)$/);
  if (!match) {
    return {tag: version.replace(/^\[(.+)\]$/, '$1'), date: ''};
  }

  const isSemver = /^\d+\.\d+\.\d+/.test(match[1]);

  return {
    tag: isSemver ? `v${match[1]}` : match[1],
    date: match[2],
  };
}

function renderItem(item: string) {
  const boldPrefixMatch = item.match(/^\*\*(.+?):\*\*\s*(.+)$/);

  if (!boldPrefixMatch) {
    return item;
  }

  return (
    <>
      <strong>{boldPrefixMatch[1]}:</strong> {boldPrefixMatch[2]}
    </>
  );
}

export default function ChangelogPage(): React.ReactElement {
  const {
    i18n: {currentLocale},
  } = useDocusaurusContext();
  const changelogReleases = getChangelogReleases(currentLocale);

  return (
    <Layout
      title={translate({message: '更新日志'})}
      description={translate({message: '查看 NyaTerm 的版本演进与功能更新记录'})}>
      <main className={styles.page}>
        <section className={styles.hero}>
          <div className="container">
            <div className={styles.heroInner}>
              <span className={styles.eyebrow}>
                <Translate>Changelog</Translate>
              </span>
              <Heading as="h1" className={styles.title}>
                <Translate>项目日志与版本演进</Translate>
              </Heading>
              <p className={styles.description}>
                {currentLocale === 'zh-CN'
                  ? '2.x 为原生 GPUI 桌面应用；1.x 记录对应早期 Tauri/WebView 实现（含历史 Web 测试版）。1.x 稳定版与 2.0.0 预览版曾并行开发；未发布部分记录最新版本标签之后的改动。'
                  : '2.x is the native GPUI desktop application. 1.x entries describe the earlier Tauri/WebView implementation, including its historical web beta. The 1.x stable line and 2.0.0 previews were developed in parallel; Unreleased covers changes after the latest release tag.'}
              </p>
            </div>
          </div>
        </section>

        <section className={styles.timelineSection}>
          <div className="container">
            <div className={styles.timeline}>
              {changelogReleases.map((release) => {
                const {tag, date} = formatVersionLabel(release.version);

                return (
                  <article key={release.version} className={styles.releaseCard}>
                    <div className={styles.releaseHeader}>
                      <div>
                        <Heading as="h2" className={styles.releaseTag}>
                          {tag}
                        </Heading>
                        {date ? <p className={styles.releaseDate}>{date}</p> : null}
                      </div>
                    </div>

                    <div className={styles.sectionList}>
                      {release.sections.map((section) => (
                        <section key={`${release.version}-${section.title}`} className={styles.sectionCard}>
                          <Heading as="h3" className={styles.sectionTitle}>
                            {section.title}
                          </Heading>
                          <ul className={styles.itemList}>
                            {section.items.map((item) => (
                              <li key={item} className={styles.item}>
                                {renderItem(item)}
                              </li>
                            ))}
                          </ul>
                        </section>
                      ))}
                    </div>
                  </article>
                );
              })}
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
