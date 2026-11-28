<p align="center">
  <img src="../static/img/nyaterm.svg" alt="NyaTerm" width="128" height="128">
</p>

<h1 align="center">NyaTerm</h1>

<p align="center">
  <em>/ˈnjaː tɜːrm/</em>
</p>

<p align="center">
  <strong>GPUI로 만든 현대적인 네이티브 원격 터미널 작업 공간.</strong><br/>
  <a href="https://nyaterm.app"><strong>nyaterm.app</strong></a> ·
  <a href="https://nyaterm.app/en/docs/"><strong>문서</strong></a>
</p>

<p align="center">
  SSH, 로컬 셸, Telnet, 시리얼, RDP, VNC, SFTP, 터널, OTP, AI 지원, 암호화 동기화를 하나의 데스크톱 클라이언트에서 사용하세요.
</p>

<p align="center">
  <a href="https://discord.gg/Tgb9ugMKdM" target="_blank"><img src="https://dcbadge.limes.pink/api/server/Tgb9ugMKdM" alt="Discord" /></a>
  &nbsp;
  <a href="https://docs.qq.com/doc/DYW5TUkZnakxOWUFW" target="_blank"><img src="https://img.shields.io/badge/WeChatGroup-Join-07C160?style=for-the-badge&logo=wechat&logoColor=white" alt="WeChat 그룹" /></a>
  <br/>
  <a href="https://linux.do" target="_blank" rel="noopener noreferrer">
    <img src="https://img.shields.io/badge/LINUX%20DO-Forum-F0B003?style=for-the-badge&logo=discourse&logoColor=white" alt="LINUX DO" />
  </a>
  <br/>
  <a href="https://nyaterm.app"><img alt="버전" src="https://img.shields.io/github/v/release/nyakang/nyaterm?style=flat-square&logo=github&color=0EA5E9&labelColor=334155"></a>
  &nbsp;
  <a href="https://github.com/nyakang/nyaterm/releases"><img alt="GitHub 다운로드 수" src="https://img.shields.io/github/downloads/nyakang/nyaterm/total?style=flat-square&logo=github&color=0EA5E9&label=Downloads&labelColor=334155"></a>
  &nbsp;
  <a href="#"><img alt="플랫폼" src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0EA5E9?style=flat-square&logo=linux&labelColor=334155"></a>
  &nbsp;
  <a href="../../LICENSE"><img alt="라이선스" src="https://img.shields.io/badge/License-Apache%202.0-0EA5E9?style=flat-square&logo=readthedocs&labelColor=334155"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="./README.zh-CN.md">简体中文</a> · <a href="./README.zh-TW.md">繁體中文</a> · <a href="./README.ja.md">日本語</a> · <strong>한국어</strong> · <a href="./README.fr.md">Français</a> · <a href="./README.pt-BR.md">Português (Brasil)</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/product-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/product-light.png">
    <img alt="NyaTerm 메인 작업 공간" src="../static/img/home/product-light.png">
  </picture>
</p>

---

<a name="ai-assistant"></a>
# AI 어시스턴트

NyaTerm은 명령 생성, 터미널 출력 설명, 오류 분석, 여러 단계의 터미널 작업을 위한 AI 어시스턴트 패널을 제공합니다.

## 주요 기능

- **Ask 모드**: 명령 생성, 선택한 출력 설명, 오류 분석 등 일회성 도움 제공
- **Agent 모드**: 활성 터미널 세션에서 관찰·판단·실행을 반복하며 여러 단계의 작업 수행
- **최근 출력 작업**: 출력을 직접 복사하지 않고 AI에 최신 터미널 출력 설명 요청
- **구조화된 명령 카드**: 위험 수준과 실행 제어를 표시하고 빠른 명령으로 저장 가능
- **승인된 명령 실행 도구**: Agent 작업 흐름에서 명령을 실행하고 터미널 작업이 끝나면 별도 단계에서 최종 답변 제공
- **인라인 터미널 출력 캡처**: Agent 실행 중 출력을 캡처하며 `Terminal Output Lines`로 줄 수 설정
- **세션 멘션**: `@`를 사용해 다른 터미널 세션을 AI 컨텍스트에 추가
- **제공업체 관리**: 내장 제공업체, 수동 모델 등록, 자격 증명 그룹, 사용자 지정 OpenAI 호환 엔드포인트 지원
- **위험 제어**: 영향이 큰 명령에 대한 승인 절차와 더 안전한 대안 제공

---

<a name="what-is-nyaterm"></a>
# NyaTerm 소개

**NyaTerm**은 SSH 중심의 운영과 다양한 터미널 작업을 위한 GPUI 기반 네이티브 데스크톱 클라이언트입니다. Rust 워크스페이스에 데스크톱 인터페이스, 터미널 렌더링, 전송 프로토콜, 영구 저장, 인증, 네트워크 도구, AI 기반 터미널 작업, 세션 가져오기/내보내기, 진단, 암호화 동기화/백업을 하나의 애플리케이션으로 통합했습니다.

- **NyaTerm은** 개발자, 시스템 관리자, DevOps 엔지니어를 위한 SSH 클라이언트입니다
- **NyaTerm은** 탭과 가로·세로 분할을 지원하는 터미널 작업 공간입니다
- **NyaTerm은** 전송 대기열과 로컬에서 편집한 후 다시 업로드하는 작업 흐름을 갖춘 SFTP 브라우저입니다
- **NyaTerm은** SSH, 로컬 터미널, Telnet, 시리얼, RDP, VNC 세션을 지원합니다

---

<a name="why-nyaterm"></a>
# NyaTerm을 선택하는 이유

NyaTerm은 하루 종일 서버, 로컬 명령, 장치, 설정 파일을 오가며 작업하는 사용자를 위해 만들어졌습니다.

- **작업 공간 중심 구성**: 탭, 분할 창, 사이드 패널, 하위 창으로 관련 터미널을 함께 관리
- **세션 안에서 원격 작업**: 세션을 벗어나지 않고 SFTP 파일 탐색, 터미널 경로 추적, 파일 전송, 원격 파일 편집 수행
- **보안을 고려한 작업 흐름**: 자격 증명, 키, 알려진 호스트, OTP, 화면 잠금, 마스터 비밀번호로 보호되는 저장소 관리
- **이동 가능한 설정**: 기존 도구에서 가져오기, 암호화된 `.nya` 백업 내보내기, WebDAV 또는 S3 호환 저장소를 통한 암호화 스냅샷 동기화
- **필요한 곳에서 활용하는 AI**: 활성 터미널 컨텍스트에서 명령 생성, 출력 확인, 승인된 다단계 작업 실행

---

<a name="features"></a>
# 기능

## 세션과 작업 공간

- SSH, 로컬 터미널, Telnet, 시리얼, RDP, VNC 세션 지원
- 가로·세로 창 분할, 탭 드래그 도킹, 레이아웃 복원을 지원하는 다중 탭 작업 공간
- RDP 및 VNC 원격 데스크톱 창. VNC는 현재 직접 TCP 연결, None/기존 VNC Auth 인증, Raw/ZRLE/Tight/Tight JPEG 프레임버퍼 업데이트, 창 크기 조절, 횟수가 제한된 재연결, Latin-1 텍스트 클립보드 교환 지원
- 저장된 연결에 폴더, 아이콘, 메타데이터, 복제, 키보드 복사, 재연결, 가져오기 기능 제공
- 명령 팔레트와 세션 빠른 전환으로 작업, 열린 세션, 저장된 연결, 새 세션 생성 항목 검색
- 메인 창의 `Background Image` 사용자 지정. `cover`/`contain`/`stretch`/`tile` 크기 모드와 `Background Content Opacity` 조절 지원
- 좌우 활동 표시줄에서 파일 탐색기, 네트워크, 보안/인증, 동기화 및 백업, AI 어시스턴트, 노트, 저장된 연결, 활성 세션, 명령 기록, 자산 모니터링, 리소스 모니터링, GPU 모니터, 프로세스 관리자, Docker 관리자 제공
- 노트 패널과 편집기에 트리 탐색, 상황에 맞는 메뉴, 자동 저장, 도구 모음, 동기화/백업에 포함되는 영구 저장 기능 제공
- 자산 모니터링 작업 공간에 그룹별 연결 보기, 탐색 경로, 표/카드 레이아웃, 리소스/GPU/NPU 상태 통합
- SSH 세션용 원격 호스트 모니터링 패널: 리소스 모니터, NVIDIA GPU 및 Ascend NPU 개요, 프로세스 관리자(signal/renice), Docker 관리자(컨테이너, 이미지, 볼륨, 네트워크, Compose)
- 세션 입력 동기화 그룹으로 입력 내용과 명령을 여러 세션에 동시에 전송
- `ssh://` URL 또는 `ssh` 명령을 붙여 넣어 연결을 저장하지 않고 일회성 SSH 연결 생성
- 설정, 새 세션 생성, 빠른 명령 편집, 원격 파일 편집, 자동 업로드 확인을 위한 하위 창
- 시스템 트레이 지원과 선택 가능한 트레이로 최소화 및 메인 창 숨기기 동작

## 터미널 사용 경험

- 결과 이동, 검색 기록, 복사/붙여넣기, 상황에 맞는 메뉴, 선택한 텍스트 작업을 지원하는 터미널 검색
- 명령 기록에 유사 검색 제안, 설정 가능한 길이 필터, 대화형 프로그램에서 제안 숨기기 기능 제공
- 이모지, 결합 문자, ZWJ 시퀀스를 위한 유니코드 자소 렌더링
- 줄 번호와 타임스탬프 여백 표시를 선택적으로 사용
- IPv4 주소, `host:port`, 압축 파일 이름에 대한 작업 링크를 선택적으로 사용
- 확장된 내장 프리셋, 사용자 지정 규칙, JSON 가져오기를 지원하는 키워드 강조 표시를 선택적으로 사용
- 터미널 확대/축소, 작업 공간 안쪽 여백, 글꼴 두께 조절, 네이티브 IME 지원, 이미지 경로 붙여넣기 처리, 터미널 지우기 기능
- 최근 출력을 설명하는 AI 바로 가기와 `Terminal Output Lines`로 줄 수를 설정할 수 있는 인라인 Agent 명령 출력
- 대량 출력 보호, 설정 가능한 스크롤백, SSH 연결 유지, 세션 녹화
- 선택한 터미널 텍스트의 온라인 검색 및 번역
- 터미널에서 직접 Zmodem 파일 전송을 수행하고 전송 대기열에 표시
- 모든 세션을 닫기 전에 확인 대화 상자 표시
- 터미널과 UI 작업의 키보드 단축키 사용자 지정 및 Telnet/시리얼 세션의 `Backspace Mode` 선택

## SFTP와 파일 작업

- SSH 세션용 내장 SFTP 파일 탐색기
- 업로드, 다운로드, 이름 바꾸기, 이동, 삭제, 속성, 새 파일/폴더, OpenSSH 호환 심볼릭 링크 작업 지원
- 폴더 업로드/다운로드, 여러 폴더 업로드 선택, 다중 선택, 편집 가능한 경로 표시줄, 터미널 작업 디렉터리와의 수동/자동 동기화
- 전송 대기열에 속도 표시, 일시 정지, 재개, 취소, 재시도, 중복 대상 처리, 타임스탬프 보존, 동시 전송 수 설정 제공
- SCP 및 디렉터리 전송 처리를 개선하고 가능한 경우 원래 속성 보존
- 원격 파일을 로컬 편집기로 열고 파일 감시 기반 자동 업로드로 저장한 변경 사항을 반영. 콘텐츠 지문을 사용해 실제 내용이 바뀐 경우에만 재업로드
- 텍스트, JSON, Markdown, CSV/TSV, 이미지, PDF를 위한 다중 탭 읽기 전용 원격 파일 미리보기. 탭별 세션 레이블, 확대/축소·회전·맞춤 제어, 페이지별 PDF 렌더링 제공
- 미리보기 로드와 디코딩을 렌더링 스레드 밖에서 수행하고 형식별 크기를 제한하여, 너무 크거나 지원되지 않는 파일은 UI를 막지 않고 명확한 메시지 표시
- SFTP 채널 동시 실행 수 제한과 일시적인 채널 열기 실패 시 자동 재시도
- Windows에서 외부 드래그 앤 드롭 업로드 지원
- Zmodem 전송의 로컬 경로를 추적하고 완료된 파일을 시스템 파일 관리자에서 표시 가능

## 보안, 인증, 네트워크

- 비밀번호 인증, 개인 키, 호스트 키 검증, 암호화된 로컬 영구 저장
- 정규식 기반 터미널 자동 입력을 지원하는 자격 증명 관리
- TOTP/HOTP, QR 가져오기, SSH 자동 입력을 지원하는 OTP 관리
- 연결별 SSH 알고리즘 설정에 호환/보안/사용자 지정 모드를 제공하고 키 교환, 암호, MAC, 호스트 키의 보안 위험 표시
- SOCKS5, HTTP, ProxyCommand 프록시 설정, 순환 방지 기능이 있는 SSH 점프 호스트, 로컬/원격/동적 터널 지원
- 로컬 X 서버가 있는 환경에서 SSH X11 포워딩 지원
- 화면 잠금, 유휴 시 앱 잠금, 마스터 비밀번호, 진단 설정, 로컬 로그 관리, 진단 번들 내보내기

## 동기화, 백업, 마이그레이션

- WebDAV, S3 호환 저장소, GitHub Gist를 통한 암호화 클라우드 동기화 및 백업
- 동기화, 백업, 암호화 가져오기/내보내기, 예약 암호화 백업 전에 마스터 비밀번호 필요
- 시작 시 동기화 확인, 지원되는 로컬 변경을 디바운스 처리한 후 자동 푸시, 자세한 상태 업데이트, 예약 백업 보존 관리
- 수동 테스트/푸시/풀/백업 작업, 원격 백업 복원, 스냅샷 단위 충돌 해결
- Xshell, MobaXterm, WindTerm, NyaTerm JSON 정의에서 세션 가져오기
- 앱 전체 설정을 암호화된 `.nya` 형식으로 가져오고 내보내 NyaTerm 설정 이동 지원

---

<a name="screenshots"></a>
# 스크린샷

## 작업 공간

탭과 분할 창을 갖춘 하나의 작업 공간에서 SSH, 로컬 셸, Telnet, 시리얼 세션을 관리하세요.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/overview-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/overview-light.png">
    <img alt="NyaTerm 작업 공간 개요" src="../static/img/home/overview-light.png">
  </picture>
</p>

## 외관과 배경 이미지

로컬 배경 이미지를 메인 창 뒤에 표시하고 `Image Sizing`, `Image Opacity`, `Background Content Opacity`를 조절할 수 있습니다. 하위 창은 단색 배경으로 가독성을 유지합니다.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/cover-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/cover-light.png">
    <img alt="NyaTerm 배경 이미지 설정" src="../static/img/home/cover-light.png">
  </picture>
</p>

## 터미널 확장 기능

터미널에서 명령 기록, 검색, 번역, 작업 링크, 타임스탬프, 키워드 강조 표시, 대량 출력 보호 기능을 사용하세요.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/terminal-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/terminal-light.png">
    <img alt="NyaTerm 터미널 기능" src="../static/img/home/terminal-light.png">
  </picture>
</p>

## 원격 파일

터미널 옆에서 SFTP 파일을 탐색하고 전송을 관리하며 로컬 편집기의 변경 사항을 원격 경로에 업로드하세요.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/files-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/files-light.png">
    <img alt="NyaTerm SFTP 파일 작업" src="../static/img/home/files-light.png">
  </picture>
</p>

## 보안 및 네트워크 도구

전용 패널에서 자격 증명, OTP, 알려진 호스트, 프록시, 점프 호스트, SSH 터널을 관리하세요.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/security-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/security-light.png">
    <img alt="NyaTerm 보안 및 네트워크 도구" src="../static/img/home/security-light.png">
  </picture>
</p>

## 동기화 및 백업

WebDAV 또는 S3 호환 저장소를 통해 암호화된 이동식 설정 스냅샷을 동기화하고 백업을 복원하세요.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/sync-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/sync-light.png">
    <img alt="NyaTerm 동기화 및 백업" src="../static/img/home/sync-light.png">
  </picture>
</p>

---

<a name="supported-platforms"></a>
# 지원 플랫폼

| 운영체제 | 지원 범위 |
| :--- | :--- |
| **Windows** | Windows 10/11, x64 / arm64 |
| **macOS** | macOS 12+, Intel / Apple Silicon |
| **Linux** | Ubuntu 20.04+, Fedora 36+, Arch Linux 및 유사 배포판 |

**glibc 2.31**을 사용하는 사용자 지정 Ubuntu 20.04 x86-64 데스크톱에는 전용
[focal 오프라인 호환 빌드](../../scripts/compat/linux-focal/README.md)를 사용하세요.
표준 Linux 릴리스 패키지는 최신 배포판에서 빌드되므로 더 새로운 glibc가 필요할 수 있습니다.
호환 빌드에는 모든 런타임 헬퍼, ABI 보고서,
시스템/GPU 우선 실행기, 명시적으로 선택하는 전용 CPU OpenGL 대체 모드가 포함됩니다.
대상 시스템에는 정상적으로 작동하는 X11 세션이 필요합니다.

[nyaterm.app](https://nyaterm.app) 또는 [Releases](https://github.com/nyakang/nyaterm/releases) 페이지에서 설치 프로그램을 다운로드하세요.

---

<a name="supported-session-types"></a>
# 지원 세션 유형

| 유형 | 일반적인 용도 | 설명 |
|------|-------------|-------|
| SSH | Linux / Unix 원격 서버 | SFTP, OTP, 리소스/GPU/프로세스/Docker 모니터링, 프록시, 점프 호스트, 터널, 연결별 알고리즘 설정 지원 |
| 로컬 터미널 | 로컬 셸 작업 | 로컬 셸 경로와 작업 디렉터리 사용 |
| Telnet | 레거시 네트워크 장치 또는 실험실 시스템 | SSH 전용 기능이 없는 경량 터미널 세션. `Backspace Mode`에서 `Ctrl+H (BS)` 또는 `DEL (0x7F)` 선택 가능 |
| 시리얼 | 라우터, 보드, 임베디드 장치 | 포트, 전송 속도, 데이터 비트, 패리티, 정지 비트, `Backspace Mode` 설정 가능 |

---

<a name="getting-started"></a>
# 시작하기

## 다운로드

[nyaterm.app](https://nyaterm.app) 또는 [Releases](https://github.com/nyakang/nyaterm/releases)에서 사용 중인 플랫폼에 맞는 최신 빌드를 다운로드하세요.

| 플랫폼 | 형식 |
|----------|--------|
| Windows | `.exe` / 포터블 `.zip` |
| macOS | `.dmg` |
| Linux | `.deb` / `.AppImage` / `.rpm` |

Windows 포터블 버전은 ZIP 파일의 압축을 풀고 `NyaTerm.exe`를 실행하면 됩니다. 데이터는 실행 파일 옆의 `data/` 디렉터리에 저장됩니다. GPUI의 **도움말 → 업데이트 확인**은 서명된 매니페스트를 사용하며, 지원되는 설치 유형에서 업데이트 다운로드와 설치를 제공합니다. Linux deb/rpm 버전은 Polkit 또는 sudo 승인 후 시스템 패키지 관리자를 통해 서명된 업데이트를 설치하고 재시작 옵션을 제공합니다. 의존성 패키지는 미리 설치되어 있어야 합니다. Homebrew 및 외부에서 관리되는 빌드는 해당 패키지 관리자를 사용합니다.

**1.x에서 마이그레이션: Windows 포터블 버전과 기존 MSI 설치는 수동 업그레이드가 필요합니다.** 먼저 앱을 종료하고 모든 데이터와 키를 백업하세요. 포터블 버전은 `data/`를 유지한 채 프로그램 파일 전체를 교체하세요. MSI 버전은 백업 후 기존 버전을 제거하고 NSIS 버전을 설치하세요. 주 실행 파일만 교체해서는 충분하지 않으며 모든 구형 설치 프로그램의 자동 마이그레이션을 보장하지 않습니다. [2.0 마이그레이션 가이드](https://nyaterm.app/en/docs/getting-started/migration-v2)를 참고하세요.

### macOS

macOS에서는 Homebrew로 NyaTerm을 설치할 수 있습니다:

```bash
brew install nyakang/nyaterm/nyaterm
```

이 명령은 [`nyakang/homebrew-nyaterm`](https://github.com/nyakang/homebrew-nyaterm) tap을 사용해 `nyaterm` cask를 설치합니다. [nyaterm.app](https://nyaterm.app) 또는 [Releases](https://github.com/nyakang/nyaterm/releases)에서 `.dmg` 설치 프로그램을 다운로드한 뒤 NyaTerm을 `/Applications`로 드래그할 수도 있습니다.

NyaTerm은 현재 Apple Developer 인증서로 서명되어 있지 않습니다. 설치 후 macOS에서 앱이 손상되었거나 열 수 없다는 메시지가 나타나면 격리 속성을 제거한 뒤 다시 여세요:

```bash
sudo xattr -cr /Applications/NyaTerm.app
```

### Arch Linux / AUR

Arch Linux에서는 AUR에서 NyaTerm을 설치할 수 있습니다:

```bash
yay -S nyaterm-bin
```

또는 `paru`를 사용하세요:

```bash
paru -S nyaterm-bin
```

AUR 저장소에서 직접 빌드할 수도 있습니다:

```bash
git clone https://aur.archlinux.org/nyaterm-bin.git
cd nyaterm-bin
makepkg -si
```

AUR 패키지: [`nyaterm-bin`](https://aur.archlinux.org/packages/nyaterm-bin)

> `nyaterm-bin`은 커뮤니티에서 유지 관리하는 AUR 바이너리 패키지입니다. AUR 패키지가 아직 최신 릴리스를 반영하지 않았다면 [Releases](https://github.com/nyakang/nyaterm/releases)에서 공식 패키지를 다운로드하세요.


## 개발 준비 사항

- [rustup](https://rustup.rs/)을 통해 설치한 Rust 안정 버전
- GPUI와 운영체제에 필요한 플랫폼 의존성 패키지

## 개발

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
cargo run -p nyaterm-app --bin nyaterm
```

## 프로젝트 구조

```text
├── crates/
│   ├── nyaterm-app/             # 네이티브 GPUI 앱 진입점과 리소스
│   ├── nyaterm-desktop/         # GPUI 앱 구성과 기능 상태
│   ├── nyaterm-core/            # UI 독립적인 도메인 모델과 정책
│   ├── nyaterm-terminal/        # 터미널 상태 머신과 파싱
│   ├── nyaterm-terminal-runtime/ # 프레임 워커, 검색, 세션 이벤트 라우팅
│   ├── nyaterm-terminal-gpui/   # GPUI 터미널 레이아웃, 입력, 렌더링
│   ├── nyaterm-transport/       # SSH, PTY, SFTP, Telnet, 시리얼, 전송
│   ├── nyaterm-store/           # 영구 저장 및 데이터베이스 호환성
│   ├── nyaterm-ui/              # 공유 GPUI 컨트롤과 테마 통합
│   ├── nyaterm-remote-desktop/  # UI 독립적인 RDP/VNC 세션과 타입이 지정된 IPC
│   ├── nyaterm-rdp-helper/      # 격리된 IronRDP 프로토콜 헬퍼 프로세스
│   ├── nyaterm-vnc-helper/      # 격리된 VNC 프로토콜 헬퍼 프로세스
│   └── nyaterm-otp/             # OTP 호환 구현
├── docs-site/                   # Docusaurus 문서 사이트
├── Cargo.toml                   # Rust 워크스페이스 정의
└── Cargo.lock
```

---

<a name="credits"></a>
# 감사의 말

NyaTerm을 가능하게 해 준 다음 프로젝트와 라이브러리에 감사드립니다:

- [WindTerm](https://github.com/kingToolbox/WindTerm) — NyaTerm의 설계와 기능에 영감을 제공
- [tabby](https://github.com/Eugeny/tabby) — 여러 설계 아이디어에 영감을 준 뛰어난 크로스 플랫폼 터미널
- [Alacritty](https://github.com/alacritty/alacritty) — NyaTerm에서 사용하는 터미널 에뮬레이션 구성 요소
- [russh](https://github.com/warp-tech/russh) — Rust로 구현된 SSH 클라이언트 및 서버

---

<a name="sponsor"></a>
# 후원

NyaTerm이 일상적인 개발, 운영 또는 원격 터미널 작업에 도움이 된다면 [후원 페이지](./en/docusaurus-plugin-content-docs/current/sponsor.md)에서 지속적인 유지 관리를 지원할 수 있습니다.

---

<a name="contributors"></a>
# 기여자

<a href="https://github.com/nyakang/nyaterm/graphs/contributors">
  <img src="https://contrib-wall.coderkang.workers.dev/image?snapshot=EEZNlyhjBOrARLki-MUhXqKk&amp;sealed_token=eyJ2IjoxLCJ0eXBlIjoic25hcHNob3QiLCJzbmFwc2hvdCI6IkVFWk5seWhqQk9yQVJMa2ktTVVoWHFLayIsImV4cCI6MTgxNTAzNjQ0NH0.UwegiwfiT3KSKQ17n5ivt24EFwAtoaHZGRwx97t0ODA&amp;sealed_github_token=A6pS93BW-53hz_Cs.8rPY0yM6koNS_UowTbu8qkWzmPLRco8HJRJO2_yp5MZH9yz9o1Ob2g5MUL4aLFK24gjbDexjdErw6eR72ruRbNcsFciYuM4DghLZJgi9V5D9f1-_xFT-sGYK6ibc1A4aWz8EyvFIQo0byPAXv-d410cJmhwsH-bqc963L8J-z0kU6oIi-2zCSszxllze23WmJsWnTJBFUBrQbllg5cLwuHm6q-BMkjoVxChLdOMVwEs" alt="기여자" />
</a>

---

## Star 기록

<a href="https://www.star-history.com/?repos=nyakang%2Fnyaterm&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&theme=dark&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <img alt="Star 기록 차트" src="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
 </picture>
</a>

---

<a name="license"></a>
# 라이선스

이 프로젝트는 [Apache License 2.0](../../LICENSE)에 따라 배포됩니다.
