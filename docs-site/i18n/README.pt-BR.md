<p align="center">
  <img src="../static/img/nyaterm.svg" alt="NyaTerm" width="128" height="128">
</p>

<h1 align="center">NyaTerm</h1>

<p align="center">
  <em>/ˈnjaː tɜːrm/</em>
</p>

<p align="center">
  <strong>Um workspace moderno de terminal remoto nativo, construído com GPUI.</strong><br/>
  <a href="https://nyaterm.app"><strong>nyaterm.app</strong></a> ·
  <a href="https://nyaterm.app/docs/"><strong>Documentação</strong></a>
</p>

<p align="center">
  SSH, shells locais, Telnet, Serial, RDP, VNC, SFTP, túneis, OTP, assistência de IA e sincronização criptografada em um único cliente desktop.
</p>

<p align="center">
  <a href="https://discord.gg/Tgb9ugMKdM" target="_blank"><img src="https://dcbadge.limes.pink/api/server/Tgb9ugMKdM" alt="Discord" /></a>
  &nbsp;
  <a href="https://docs.qq.com/doc/DYW5TUkZnakxOWUFW" target="_blank"><img src="https://img.shields.io/badge/Grupo%20WeChat-Entrar-07C160?style=for-the-badge&logo=wechat&logoColor=white" alt="Grupo WeChat" /></a>
  <br/>
  <a href="https://linux.do" target="_blank" rel="noopener noreferrer">
    <img src="https://img.shields.io/badge/LINUX%20DO-Fórum-F0B003?style=for-the-badge&logo=discourse&logoColor=white" alt="LINUX DO" />
  </a>
  <br/>
  <a href="https://nyaterm.app"><img alt="Versão" src="https://img.shields.io/github/v/release/nyakang/nyaterm?style=flat-square&logo=github&color=0EA5E9&labelColor=334155"></a>
  &nbsp;
  <a href="https://github.com/nyakang/nyaterm/releases"><img alt="Downloads no GitHub" src="https://img.shields.io/github/downloads/nyakang/nyaterm/total?style=flat-square&logo=github&color=0EA5E9&label=Downloads&labelColor=334155"></a>
  &nbsp;
  <a href="#"><img alt="Plataforma" src="https://img.shields.io/badge/Plataforma-Windows%20%7C%20macOS%20%7C%20Linux-0EA5E9?style=flat-square&logo=linux&labelColor=334155"></a>
  &nbsp;
  <a href="LICENSE"><img alt="Licença" src="https://img.shields.io/badge/Licen%C3%A7a-Apache%202.0-0EA5E9?style=flat-square&logo=readthedocs&labelColor=334155"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="./README.zh-CN.md">简体中文</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/product-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/product-light.png">
    <img alt="Workspace principal do NyaTerm" src="../static/img/home/product-light.png">
  </picture>
</p>

---

<a name="ai-assistant"></a>
# Assistente de IA

O NyaTerm inclui um painel de Assistente de IA para geração de comandos, explicação da saída do terminal, análise de erros e fluxos de trabalho multietapa no terminal.

## O que ele faz

- **Modo Pergunta** para ajuda pontual, como gerar comandos, explicar a saída selecionada e analisar erros
- **Modo Agente** para trabalhos multietapa usando um ciclo observar-decidir-executar na sessão de terminal ativa
- **Ações na saída recente** para pedir à IA que explique a última saída do terminal sem copiá-la manualmente
- **Cartões de comando estruturados**, com níveis de risco, controles de execução e opção de salvar como comando rápido
- **Ferramentas de execução de comandos aprovados** para fluxos do Agente, com uma etapa separada de resposta final após a conclusão do trabalho no terminal
- **Captura embutida do terminal** durante a execução do Agente, com `Terminal Output Lines` configurável
- **Menções de sessão** com `@` para trazer outras sessões de terminal para o contexto da IA
- **Gerenciamento de provedores** para provedores integrados, entradas manuais de modelos, grupos de credenciais e endpoints personalizados compatíveis com OpenAI
- **Controle de risco** para comandos de alto impacto, incluindo portões de aprovação e alternativas mais seguras

---

<a name="what-is-nyaterm"></a>
# O que é o NyaTerm

**NyaTerm** é um cliente desktop nativo em GPUI para operações centradas em SSH e fluxos de terminal mistos. Seu workspace em Rust combina a interface desktop, renderização de terminal, protocolos de transporte, persistência, autenticação, ferramentas de rede, ações de terminal assistidas por IA, importação/exportação de sessões, diagnósticos e sincronização/backup criptografados em uma única aplicação.

- **NyaTerm é** um cliente SSH para desenvolvedores, administradores de sistema e engenheiros DevOps
- **NyaTerm é** um workspace de terminal com abas, divisões horizontais e verticais
- **NyaTerm é** um navegador SFTP com fila de transferências e fluxo de editar localmente e enviar de volta
- **NyaTerm oferece suporte** a sessões SSH, Terminal Local, Telnet, Serial, RDP e VNC

---

<a name="why-nyaterm"></a>
# Por que usar o NyaTerm

O NyaTerm foi criado para quem passa o dia alternando entre servidores, comandos locais, dispositivos e arquivos de configuração.

- **Foco no workspace** — mantenha terminais relacionados juntos com abas, painéis divididos, painéis laterais e janelas filhas
- **Operações remotas no contexto** — navegue arquivos SFTP, acompanhe caminhos do terminal, execute transferências e edite arquivos remotos sem sair da sessão
- **Fluxos conscientes de segurança** — gerencie credenciais, chaves, hosts conhecidos, OTP, tela de bloqueio e armazenamento protegido por senha mestra
- **Configuração portátil** — importe de ferramentas existentes, exporte backups `.nya` criptografados e sincronize snapshots criptografados via WebDAV ou armazenamento compatível com S3
- **IA onde é útil** — gere comandos, inspecione a saída e execute ações aprovadas multietapa a partir do contexto do terminal ativo

---

<a name="features"></a>
# Recursos

## Sessões e workspace

- Suporte a sessões SSH, Terminal Local, Telnet, Serial, RDP e VNC
- Workspace com várias abas, divisões horizontais e verticais, encaixe de abas por arraste e restauração de layout
- Painéis de área de trabalho remota RDP e VNC; o VNC atualmente oferece suporte a TCP direto, autenticação None / VNC Auth clássica, atualizações de framebuffer Raw / ZRLE / Tight / Tight JPEG, escalonamento de janela, reconexões limitadas e troca de área de transferência de texto para texto Latin-1
- Conexões salvas com pastas, ícones, metadados, duplicação, cópia por teclado, reconexão e fluxos de importação
- Paleta de comandos e trocador rápido de sessões para localizar ações, sessões abertas, conexões salvas e pontos de entrada de novas sessões
- Personalização de `Background Image` na janela principal com dimensionamento `cover` / `contain` / `stretch` / `tile` e `Background Content Opacity` ajustável
- Barras de atividade esquerda e direita para explorador de arquivos, rede, Segurança/Autenticação, Sincronização e Backup, Assistente de IA, Notas, conexões salvas, sessões ativas, histórico de comandos, monitoramento de ativos, monitoramento de recursos, monitor GPU, gerenciador de processos e gerenciador Docker
- Painel e editor de notas com navegação em árvore, menus de contexto, autosave, controles na barra de ferramentas e persistência integrada à sincronização/backup
- Workspace de monitoramento de ativos com visualizações agrupadas por conexão, navegação por trilha, layouts em tabela/cartão e integração com status de recursos/GPU/NPU
- Painéis de monitoramento de hosts remotos para sessões SSH: monitor de recursos, visões gerais de GPU NVIDIA e NPU Ascend, gerenciador de processos (sinal/renice) e gerenciador Docker (contêineres, imagens, volumes, redes, Compose)
- Grupos de sincronização de entrada de sessão para transmitir o que você digita e os comandos enviados para várias sessões ao mesmo tempo
- Links SSH temporários para conexões pontuais a partir de uma URL `ssh://` ou comando `ssh` colado, sem salvar a conexão
- Janelas filhas para configurações, criação de nova sessão, edição de comandos rápidos, edição de arquivos remotos e prompts de upload automático
- Suporte à bandeja do sistema com opção de minimizar para a bandeja e ocultar a janela principal

## Experiência do terminal

- Busca no terminal com navegação de resultados, histórico de buscas, copiar/colar, menus de contexto e ações sobre o texto selecionado
- Histórico de comandos com sugestões difusas, filtros de tamanho configuráveis e supressão em programas interativos
- Renderização de grafemas Unicode para emoji, marcas combinantes e sequências ZWJ
- Medalha opcional de números de linha e carimbos de data/hora
- Links de ação opcionais para endereços IPv4, `host:porta` e nomes de arquivos compactados
- Destaque de palavras-chave opcional com predefinições ampliadas, regras personalizadas e importação JSON
- Zoom do terminal, espaçamento do workspace, controles de peso da fonte, suporte nativo a IME, tratamento de colagem de caminhos de imagem e ação de limpar o terminal
- Atalhos de IA para explicar a saída recente, além de saída de comandos do Agente embutida com `Terminal Output Lines` configurável
- Proteção contra saídas muito grandes, scrollback configurável, keep-alive SSH e gravação de sessão
- Busca online e tradução do texto selecionado no terminal
- Transferência de arquivos Zmodem diretamente do terminal, exibida na fila de transferências
- Diálogo de confirmação antes de fechar todas as sessões
- Atalhos de teclado personalizáveis para ações do terminal e da interface, incluindo a seleção de `Backspace Mode` para sessões Telnet e Serial

## Fluxos de SFTP e arquivos

- Explorador de arquivos SFTP integrado para sessões SSH
- Upload, download, renomear, mover, excluir, propriedades, novo arquivo/pasta e ações de link simbólico compatíveis com OpenSSH
- Upload/download de pastas, seleção de várias pastas para upload, seleção múltipla, barra de caminho editável e sincronização manual/automática com o diretório atual do terminal
- Fila de transferências com exibição de velocidade, pausa, retomada, cancelamento, nova tentativa, tratamento de destinos duplicados, preservação de carimbos de data/hora e concorrência configurável
- Tratamento aprimorado de SCP e transferência de diretórios com preservação das propriedades originais quando disponível
- Abra arquivos remotos em um editor local e envie as alterações salvas de volta pelo fluxo de upload automático baseado em observação de arquivos, com impressão digital de conteúdo para que apenas mudanças reais disparem o reenvio
- Visualização remota de arquivos em várias abas, somente leitura, para texto, JSON, Markdown, CSV/TSV, imagens e PDF, com rótulos de sessão por aba, controles de zoom/rotação/ajuste e renderização paginada de PDF
- Carregamento e decodificação da visualização fora da thread de renderização, com limites de tamanho por formato, para que arquivos grandes ou não suportados mostrem uma mensagem clara em vez de travar a interface
- Limite de concorrência de canais SFTP e nova tentativa automática em falhas transitórias de abertura de canal
- Suporte a upload por arrastar e soltar externo no Windows
- Transferências Zmodem registram caminhos locais e podem revelar os arquivos concluídos no gerenciador de arquivos do sistema

## Segurança, autenticação e rede

- Autenticação por senha, chaves privadas, verificação de chave do host e persistência local criptografada
- Gerenciamento de credenciais com preenchimento automático no terminal baseado em expressões regulares
- Gerenciamento de OTP com TOTP/HOTP, importação por QR e preenchimento automático em SSH
- Preferências de algoritmos SSH por conexão com modos Compatível / Seguro / Personalizado e rótulos de risco de segurança para troca de chaves, cifras, MACs e chaves de host
- Configurações de proxy, incluindo SOCKS5, HTTP e ProxyCommand; hosts de salto SSH com prevenção de ciclos; túneis locais / remotos / dinâmicos
- Encaminhamento X11 do SSH para ambientes com servidor X local
- Bloqueio de tela, bloqueio automático por inatividade, senha mestra, configurações de diagnóstico, gerenciamento de logs locais e exportação de pacote de diagnósticos

## Sincronização, backup e migração

- Sincronização e backup em nuvem criptografados via WebDAV, armazenamento compatível com S3 e GitHub Gist
- Senha mestra obrigatória antes de sincronizar, fazer backup, importar/exportar com criptografia ou executar backups criptografados agendados
- Verificações de sincronização na inicialização, autoenvio com debounce após alterações locais compatíveis, atualizações detalhadas de status e retenção de backups agendados
- Ações manuais de teste / envio / recebimento / backup, restauração de backup remoto e resolução de conflitos no nível de snapshot
- Importação de sessões de Xshell, MobaXterm, WindTerm e definições JSON do NyaTerm
- Importação/exportação `.nya` criptografada de todo o aplicativo para configuração portátil do NyaTerm

---

<a name="screenshots"></a>
# Capturas de tela

## Workspace

Gerencie sessões SSH, shell local, Telnet e Serial em um único workspace com abas e painéis divididos.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/overview-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/overview-light.png">
    <img alt="Visão geral do workspace do NyaTerm" src="../static/img/home/overview-light.png">
  </picture>
</p>

## Aparência e imagem de fundo

Use um papel de parede local atrás da janela principal, ajuste `Image Sizing`, `Image Opacity` e `Background Content Opacity`, e mantenha as janelas filhas legíveis com superfícies sólidas.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/cover-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/cover-light.png">
    <img alt="Personalização da imagem de fundo do NyaTerm" src="../static/img/home/cover-light.png">
  </picture>
</p>

## Melhorias do terminal

Use histórico de comandos, busca, tradução, links de ação, carimbos de data/hora, destaque de palavras-chave e proteção contra saídas grandes no terminal.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/terminal-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/terminal-light.png">
    <img alt="Recursos do terminal do NyaTerm" src="../static/img/home/terminal-light.png">
  </picture>
</p>

## Arquivos remotos

Navegue arquivos SFTP ao lado do terminal, gerencie transferências e envie alterações do editor local de volta ao caminho remoto.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/files-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/files-light.png">
    <img alt="Fluxo de arquivos SFTP do NyaTerm" src="../static/img/home/files-light.png">
  </picture>
</p>

## Ferramentas de segurança e rede

Gerencie credenciais, OTP, hosts conhecidos, proxies, hosts de salto e túneis SSH em painéis dedicados.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/security-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/security-light.png">
    <img alt="Ferramentas de segurança e rede do NyaTerm" src="../static/img/home/security-light.png">
  </picture>
</p>

## Sincronização e backup

Sincronize snapshots de configuração portáteis criptografados e restaure backups via WebDAV ou armazenamento compatível com S3.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/sync-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/sync-light.png">
    <img alt="Sincronização e backup do NyaTerm" src="../static/img/home/sync-light.png">
  </picture>
</p>

---

<a name="supported-platforms"></a>
# Plataformas com suporte

| Sistema operacional | Suporte |
| :--- | :--- |
| **Windows** | Windows 10/11, x64 / arm64 |
| **macOS** | macOS 12+, Intel / Apple Silicon |
| **Linux** | Ubuntu 20.04+, Fedora 36+, Arch Linux e distribuições semelhantes |

Para desktops Ubuntu 20.04 x86-64 personalizados com **glibc 2.31**, use o
[build de compatibilidade offline focal](scripts/compat/linux-focal/README.md).
Os pacotes Linux padrão são compilados em distribuições mais recentes e podem exigir
uma versão mais nova do glibc. O build de compatibilidade inclui todos os auxiliares de
runtime, um relatório ABI, um inicializador que prioriza sistema/GPU e uma opção explícita
de fallback CPU OpenGL privativo; a máquina de destino precisa de uma sessão X11 funcional.

Baixe os instaladores em [nyaterm.app](https://nyaterm.app) ou na página de [Releases](https://github.com/nyakang/nyaterm/releases).

---

<a name="supported-session-types"></a>
# Tipos de sessão com suporte

| Tipo | Uso típico | Observações |
|------|------------|-------------|
| SSH | Servidores remotos Linux / Unix | Oferece suporte a SFTP, OTP, monitoramento de recursos / GPU / processos / Docker, proxy, host de salto, túneis e preferências de algoritmos por conexão |
| Terminal Local | Fluxos de shell local | Usa o caminho do shell e o diretório de trabalho locais |
| Telnet | Dispositivos de rede legados ou sistemas de laboratório | Sessão de terminal leve sem recursos exclusivos do SSH, com `Backspace Mode` para `Ctrl+H (BS)` ou `DEL (0x7F)` |
| Serial | Roteadores, placas e dispositivos embarcados | Porta, taxa de baud, bits de dados, paridade, bits de parada e `Backspace Mode` configuráveis |

---

<a name="getting-started"></a>
# Começando

## Download

Baixe a versão mais recente para sua plataforma em [nyaterm.app](https://nyaterm.app) ou em [Releases](https://github.com/nyakang/nyaterm/releases).

| Plataforma | Formato |
|------------|---------|
| Windows | `.exe` / `.zip` portátil |
| macOS | `.dmg` |
| Linux | `.deb` / `.AppImage` / `.rpm` |

Na edição portátil do Windows, extraia o ZIP e execute `NyaTerm.exe`; os dados ficam no diretório `data/` ao lado. O comando GPUI **Ajuda → Verificar atualizações** usa manifestos assinados e oferece suporte ao download e à instalação de atualizações para os tipos de instalação compatíveis. Instalações via Homebrew e deb/rpm usam seu respectivo gerenciador de pacotes.

**Migrando da 1.x: as instalações portáteis do Windows e as antigas instalações MSI exigem atualização manual.** Antes de tudo, saia do aplicativo e faça backup de todos os dados e chaves. Substitua todo o conteúdo do programa portátil preservando `data/`; no caso do MSI, faça backup, desinstale a versão antiga e instale a versão NSIS. Substituir apenas o executável principal não é suficiente, e não há promessa de migração automática para todos os instaladores legados. Consulte o [guia de migração para 2.0](https://nyaterm.app/en/docs/getting-started/migration-v2).

### macOS

Usuários de macOS podem instalar o NyaTerm com Homebrew:

```bash
brew install nyakang/nyaterm/nyaterm
```

Isso usa o tap [`nyakang/homebrew-nyaterm`](https://github.com/nyakang/homebrew-nyaterm) e instala o cask `nyaterm`. Você também pode baixar o instalador `.dmg` em [nyaterm.app](https://nyaterm.app) ou em [Releases](https://github.com/nyakang/nyaterm/releases) e arrastar o NyaTerm para `/Applications`.

Atualmente, o NyaTerm não é assinado com um certificado de desenvolvedor da Apple. Se o macOS informar que o aplicativo está danificado ou não pode ser aberto após a instalação, remova o atributo de quarentena e abra novamente:

```bash
sudo xattr -cr /Applications/NyaTerm.app
```

### Arch Linux / AUR

Usuários de Arch Linux podem instalar o NyaTerm pela AUR:

```bash
yay -S nyaterm-bin
```

Ou com `paru`:

```bash
paru -S nyaterm-bin
```

Você também pode compilar manualmente a partir do repositório da AUR:

```bash
git clone https://aur.archlinux.org/nyaterm-bin.git
cd nyaterm-bin
makepkg -si
```

Pacote da AUR: [`nyaterm-bin`](https://aur.archlinux.org/packages/nyaterm-bin)

> `nyaterm-bin` é um pacote binário da AUR mantido pela comunidade. Se o pacote da AUR ainda não tiver acompanhado a versão mais recente, baixe o pacote oficial em [Releases](https://github.com/nyakang/nyaterm/releases).

## Pré-requisitos para desenvolvimento

- Rust stable via [rustup](https://rustup.rs/)
- Dependências da plataforma exigidas pelo GPUI e pelo seu sistema operacional

## Desenvolvimento

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
cargo run -p nyaterm-app --bin nyaterm
```

## Estrutura do projeto

```text
├── crates/
│   ├── nyaterm-app/             # Ponto de entrada e assets da aplicação GPUI nativa
│   ├── nyaterm-desktop/         # Composição da aplicação GPUI e estado dos recursos
│   ├── nyaterm-core/            # Modelos de domínio e políticas independentes da UI
│   ├── nyaterm-terminal/        # Máquina de estados e análise do terminal
│   ├── nyaterm-terminal-runtime/ # Worker de quadros, busca e roteamento de eventos de sessão
│   ├── nyaterm-terminal-gpui/   # Layout, entrada e pintura do terminal em GPUI
│   ├── nyaterm-transport/       # SSH, PTY, SFTP, Telnet, Serial e transferências
│   ├── nyaterm-store/           # Persistência e compatibilidade do banco de dados
│   ├── nyaterm-ui/              # Controles GPUI compartilhados e integração com tema
│   ├── nyaterm-remote-desktop/  # Sessões RDP/VNC e IPC tipado, independentes da UI
│   ├── nyaterm-rdp-helper/      # Processo auxiliar isolado do protocolo IronRDP
│   ├── nyaterm-vnc-helper/      # Processo auxiliar isolado do protocolo VNC
│   └── nyaterm-otp/             # Implementação compatível de OTP
├── docs-site/                   # Site de documentação Docusaurus
├── Cargo.toml                   # Definição do workspace Rust
└── Cargo.lock
```

---

<a name="credits"></a>
# Créditos

Obrigado aos seguintes projetos e bibliotecas que tornam o NyaTerm possível:

- [WindTerm](https://github.com/kingToolbox/WindTerm) — inspirou o design e os recursos do NyaTerm
- [tabby](https://github.com/Eugeny/tabby) — um excelente terminal multiplataforma que inspirou várias decisões de design
- [Alacritty](https://github.com/alacritty/alacritty) — componentes de emulação de terminal usados pelo NyaTerm
- [russh](https://github.com/warp-tech/russh) — uma implementação cliente/servidor SSH em Rust

---

<a name="sponsor"></a>
# Patrocínio

Se o NyaTerm ajuda no seu desenvolvimento, operações ou fluxo de terminal remoto diário, você pode apoiar a manutenção contínua pela [página de patrocínio](./docs-site/i18n/en/docusaurus-plugin-content-docs/current/sponsor.md).

---

<a name="contributors"></a>
# Colaboradores

<a href="https://github.com/nyakang/nyaterm/graphs/contributors">
  <img src="https://contrib-wall.coderkang.workers.dev/image?snapshot=EEZNlyhjBOrARLki-MUhXqKk&amp;sealed_token=eyJ2IjoxLCJ0eXBlIjoic25hcHNob3QiLCJzbmFwc2hvdCI6IkVFWk5seWhqQk9yQVJMa2ktTVVoWHFLayIsImV4cCI6MTgxNTAzNjQ0NH0.UwegiwfiT3KSKQ17n5ivt24EFwAtoaHZGRwx97t0ODA&amp;sealed_github_token=A6pS93BW-53hz_Cs.8rPY0yM6koNS_UowTbu8qkWzmPLRco8HJRJO2_yp5MZH9yz9o1Ob2g5MUL4aLFK24gjbDexjdErw6eR72ruRbNcsFciYuM4DghLZJgi9V5D9f1-_xFT-sGYK6ibc1A4aWz8EyvFIQo0byPAXv-d410cJmhwsH-bqc963L8J-z0kU6oIi-2zCSszxllze23WmJsWnTJBFUBrQbllg5cLwuHm6q-BMkjoVxChLdOMVwEs" alt="Colaboradores" />
</a>

---

## Histórico de estrelas

<a href="https://www.star-history.com/?repos=nyakang%2Fnyaterm&type=date&legend=top-left">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&theme=dark&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
    <img alt="Gráfico do histórico de estrelas" src="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
  </picture>
</a>

---

<a name="license"></a>
# Licença

Este projeto é licenciado sob a [Apache License 2.0](LICENSE).
