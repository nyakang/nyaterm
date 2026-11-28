<p align="center">
  <img src="../static/img/nyaterm.svg" alt="NyaTerm" width="128" height="128">
</p>

<h1 align="center">NyaTerm</h1>

<p align="center">
  <em>/ˈnjaː tɜːrm/</em>
</p>

<p align="center">
  <strong>Un espace de travail moderne de terminal distant, natif et construit avec GPUI.</strong><br/>
  <a href="https://nyaterm.app"><strong>nyaterm.app</strong></a> ·
  <a href="https://nyaterm.app/en/docs/"><strong>Documentation</strong></a>
</p>

<p align="center">
  SSH, shells locaux, Telnet, série, RDP, VNC, SFTP, tunnels, OTP, assistance IA et synchronisation chiffrée dans un seul client de bureau.
</p>

<p align="center">
  <a href="https://discord.gg/Tgb9ugMKdM" target="_blank"><img src="https://dcbadge.limes.pink/api/server/Tgb9ugMKdM" alt="Discord" /></a>
  &nbsp;
  <a href="https://docs.qq.com/doc/DYW5TUkZnakxOWUFW" target="_blank"><img src="https://img.shields.io/badge/WeChatGroup-Join-07C160?style=for-the-badge&logo=wechat&logoColor=white" alt="Groupe WeChat" /></a>
  <br/>
  <a href="https://linux.do" target="_blank" rel="noopener noreferrer">
    <img src="https://img.shields.io/badge/LINUX%20DO-Forum-F0B003?style=for-the-badge&logo=discourse&logoColor=white" alt="LINUX DO" />
  </a>
  <br/>
  <a href="https://nyaterm.app"><img alt="Version" src="https://img.shields.io/github/v/release/nyakang/nyaterm?style=flat-square&logo=github&color=0EA5E9&labelColor=334155"></a>
  &nbsp;
  <a href="https://github.com/nyakang/nyaterm/releases"><img alt="Téléchargements GitHub" src="https://img.shields.io/github/downloads/nyakang/nyaterm/total?style=flat-square&logo=github&color=0EA5E9&label=Downloads&labelColor=334155"></a>
  &nbsp;
  <a href="#"><img alt="Plateformes" src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0EA5E9?style=flat-square&logo=linux&labelColor=334155"></a>
  &nbsp;
  <a href="../../LICENSE"><img alt="Licence" src="https://img.shields.io/badge/License-Apache%202.0-0EA5E9?style=flat-square&logo=readthedocs&labelColor=334155"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="./README.zh-CN.md">简体中文</a> · <a href="./README.zh-TW.md">繁體中文</a> · <a href="./README.ja.md">日本語</a> · <a href="./README.ko.md">한국어</a> · <strong>Français</strong> · <a href="./README.pt-BR.md">Português (Brasil)</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/product-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/product-light.png">
    <img alt="Espace de travail principal de NyaTerm" src="../static/img/home/product-light.png">
  </picture>
</p>

---

<a name="ai-assistant"></a>
# Assistant IA

NyaTerm comprend un panneau d'assistance IA pour générer des commandes, expliquer les sorties du terminal, analyser les erreurs et réaliser des tâches en plusieurs étapes dans le terminal.

## Fonctionnalités de l'assistant

- **Mode Ask** pour une aide ponctuelle : générer des commandes, expliquer une sortie sélectionnée ou analyser des erreurs
- **Mode Agent** pour les tâches en plusieurs étapes, avec une boucle d'observation, de décision et d'exécution sur la session de terminal active
- **Actions sur les sorties récentes** pour demander à l'IA d'expliquer les dernières sorties du terminal sans les copier manuellement
- **Cartes de commande structurées** avec niveau de risque, commandes d'exécution et possibilité d'enregistrer une commande rapide
- **Outils d'exécution de commandes approuvées** pour les tâches de l'Agent, avec une étape distincte de réponse finale après les opérations dans le terminal
- **Capture intégrée des sorties du terminal** pendant l'exécution de l'Agent, avec un nombre de lignes configurable via `Terminal Output Lines`
- **Mentions de sessions** avec `@` pour inclure d'autres sessions de terminal dans le contexte de l'IA
- **Gestion des fournisseurs** : fournisseurs intégrés, ajout manuel de modèles, groupes d'identifiants et points de terminaison personnalisés compatibles avec OpenAI
- **Contrôle des risques** pour les commandes à fort impact, avec étapes d'approbation et propositions plus sûres

---

<a name="what-is-nyaterm"></a>
# Qu'est-ce que NyaTerm ?

**NyaTerm** est un client de bureau natif basé sur GPUI, conçu pour l'administration via SSH et les usages variés du terminal. Son espace de travail Rust réunit dans une seule application l'interface de bureau, le rendu du terminal, les protocoles de transport, la persistance, l'authentification, les outils réseau, les actions assistées par IA, l'import/export de sessions, les diagnostics et la synchronisation/sauvegarde chiffrée.

- **NyaTerm est** un client SSH destiné aux développeurs, administrateurs système et ingénieurs DevOps
- **NyaTerm est** un espace de travail de terminal avec onglets et divisions horizontales et verticales
- **NyaTerm est** un navigateur SFTP doté d'une file de transferts et d'un flux d'édition locale avec renvoi des modifications
- **NyaTerm prend en charge** les sessions SSH, terminal local, Telnet, série, RDP et VNC

---

<a name="why-nyaterm"></a>
# Pourquoi choisir NyaTerm ?

NyaTerm s'adresse aux personnes qui passent toute la journée des serveurs aux commandes locales, aux appareils et aux fichiers de configuration.

- **Un espace de travail organisé** — regroupez les terminaux liés grâce aux onglets, volets divisés, panneaux latéraux et fenêtres secondaires
- **Des opérations distantes dans leur contexte** — parcourez les fichiers SFTP, suivez les chemins du terminal, transférez et modifiez des fichiers distants sans quitter la session
- **Des usages attentifs à la sécurité** — gérez identifiants, clés, hôtes connus, OTP, verrouillage de l'écran et stockage protégé par un mot de passe principal
- **Une configuration portable** — importez depuis vos outils existants, exportez des sauvegardes `.nya` chiffrées et synchronisez des instantanés chiffrés via WebDAV ou un stockage compatible S3
- **L'IA là où elle est utile** — générez des commandes, examinez les sorties et exécutez des actions approuvées en plusieurs étapes à partir du contexte du terminal actif

---

<a name="features"></a>
# Fonctionnalités

## Sessions et espace de travail

- Prise en charge des sessions SSH, terminal local, Telnet, série, RDP et VNC
- Espace de travail à plusieurs onglets avec divisions horizontales et verticales, ancrage des onglets par glisser-déposer et restauration de la disposition
- Volets de bureau à distance RDP et VNC ; VNC prend actuellement en charge les connexions TCP directes, l'authentification None / VNC Auth classique, les mises à jour du tampon d'image Raw / ZRLE / Tight / Tight JPEG, la mise à l'échelle de la fenêtre, un nombre limité de reconnexions et l'échange de texte Latin-1 via le presse-papiers
- Connexions enregistrées avec dossiers, icônes, métadonnées, duplication, copie au clavier, reconnexion et import
- Palette de commandes et sélecteur rapide de sessions pour trouver les actions, sessions ouvertes, connexions enregistrées et points d'entrée de création de session
- Personnalisation de l'image de fond de la fenêtre principale via `Background Image`, avec les modes `cover` / `contain` / `stretch` / `tile` et le réglage `Background Content Opacity`
- Barres d'activité à gauche et à droite donnant accès à l'explorateur de fichiers, au réseau, à la sécurité/authentification, à la synchronisation et aux sauvegardes, à l'assistant IA, aux notes, aux connexions enregistrées, aux sessions actives, à l'historique des commandes, au suivi du parc et des ressources, au suivi GPU, au gestionnaire de processus et au gestionnaire Docker
- Panneau et éditeur de notes avec navigation arborescente, menus contextuels, enregistrement automatique, barre d'outils et persistance intégrée à la synchronisation et aux sauvegardes
- Espace de suivi du parc avec vues de connexions groupées, fil d'Ariane, affichage en tableau ou en cartes et intégration de l'état des ressources, GPU et NPU
- Panneaux de suivi des hôtes distants pour les sessions SSH : moniteur de ressources, vues d'ensemble NVIDIA GPU et Ascend NPU, gestionnaire de processus (signal/renice) et gestionnaire Docker (conteneurs, images, volumes, réseaux, Compose)
- Groupes de synchronisation des entrées pour diffuser le texte saisi et les commandes vers plusieurs sessions à la fois
- Connexions SSH temporaires à partir d'une URL `ssh://` ou d'une commande `ssh` collée, sans enregistrer la connexion
- Fenêtres secondaires pour les paramètres, la création de sessions, l'édition de commandes rapides et de fichiers distants, ainsi que les confirmations de téléversement automatique
- Prise en charge de la zone de notification, avec réduction dans cette zone et masquage de la fenêtre principale en option

## Expérience du terminal

- Recherche dans le terminal avec navigation entre les résultats, historique de recherche, copier/coller, menus contextuels et actions sur le texte sélectionné
- Historique des commandes avec suggestions par correspondance approximative, filtres de longueur configurables et désactivation des suggestions dans les programmes interactifs
- Rendu des graphèmes Unicode pour les emoji, signes combinatoires et séquences ZWJ
- Affichage facultatif des numéros de ligne et horodatages dans la marge
- Liens d'action facultatifs pour les adresses IPv4, `host:port` et noms de fichiers d'archives
- Surlignage facultatif des mots-clés avec préréglages intégrés enrichis, règles personnalisées et import JSON
- Zoom du terminal, marges internes de l'espace de travail, épaisseur de police, prise en charge native des IME, gestion du collage de chemins d'images et action d'effacement du terminal
- Raccourcis IA pour expliquer les sorties récentes et affichage intégré des sorties de commandes de l'Agent, avec le réglage `Terminal Output Lines`
- Protection contre les sorties volumineuses, historique de défilement configurable, maintien des connexions SSH et enregistrement des sessions
- Recherche en ligne et traduction du texte sélectionné dans le terminal
- Transferts de fichiers Zmodem directement depuis le terminal, visibles dans la file de transferts
- Boîte de confirmation avant la fermeture de toutes les sessions
- Raccourcis clavier personnalisables pour le terminal et l'interface, avec sélection de `Backspace Mode` pour les sessions Telnet et série

## SFTP et gestion des fichiers

- Explorateur de fichiers SFTP intégré aux sessions SSH
- Téléversement, téléchargement, renommage, déplacement, suppression, propriétés, création de fichiers/dossiers et opérations sur les liens symboliques compatibles avec OpenSSH
- Téléversement/téléchargement de dossiers, sélection de plusieurs dossiers à téléverser, sélection multiple, barre de chemin modifiable et synchronisation manuelle/automatique avec le répertoire courant du terminal
- File de transferts avec affichage du débit, pause, reprise, annulation, nouvelle tentative, gestion des destinations en doublon, conservation des horodatages et nombre de transferts simultanés configurable
- Gestion améliorée des transferts SCP et de répertoires, avec conservation des propriétés d'origine lorsque cela est possible
- Ouverture des fichiers distants dans un éditeur local et renvoi des modifications enregistrées grâce à la surveillance des fichiers ; les empreintes de contenu ne déclenchent un nouvel envoi que si le contenu a réellement changé
- Aperçu distant en lecture seule et à plusieurs onglets pour le texte, JSON, Markdown, CSV/TSV, les images et les PDF, avec étiquettes de session par onglet, zoom, rotation, ajustement à la fenêtre et rendu des PDF page par page
- Chargement et décodage des aperçus hors du thread de rendu, avec des limites de taille par format : les fichiers trop volumineux ou non pris en charge affichent un message clair sans bloquer l'interface
- Limitation du nombre de canaux SFTP simultanés et nouvelle tentative automatique lors d'échecs temporaires d'ouverture de canal
- Téléversement par glisser-déposer depuis l'extérieur de l'application sous Windows
- Suivi des chemins locaux pour les transferts Zmodem et possibilité d'afficher les fichiers terminés dans le gestionnaire de fichiers du système

## Sécurité, authentification et réseau

- Authentification par mot de passe, clés privées, vérification des clés d'hôte et persistance locale chiffrée
- Gestion des identifiants avec remplissage automatique du terminal fondé sur des expressions régulières
- Gestion OTP avec TOTP/HOTP, import par QR et remplissage automatique SSH
- Préférences d'algorithmes SSH par connexion avec modes Compatible / Sécurisé / Personnalisé et indication des risques pour l'échange de clés, les chiffrements, les MAC et les clés d'hôte
- Configuration de proxys SOCKS5, HTTP et ProxyCommand ; hôtes de rebond SSH avec prévention des cycles ; tunnels locaux, distants et dynamiques
- Transfert X11 via SSH pour les environnements dotés d'un serveur X local
- Verrouillage de l'écran, verrouillage de l'application en cas d'inactivité, mot de passe principal, paramètres de diagnostic, gestion des journaux locaux et export d'archives de diagnostic

## Synchronisation, sauvegarde et migration

- Synchronisation et sauvegarde chiffrées dans le cloud via WebDAV, stockage compatible S3 et GitHub Gist
- Mot de passe principal requis avant la synchronisation, les sauvegardes, l'import/export chiffré et les sauvegardes chiffrées planifiées
- Vérification de la synchronisation au démarrage, envoi automatique temporisé après les modifications locales prises en charge, suivi détaillé de l'état et politique de conservation des sauvegardes planifiées
- Actions manuelles de test, d'envoi, de récupération et de sauvegarde, restauration de sauvegardes distantes et résolution des conflits au niveau des instantanés
- Import de sessions depuis Xshell, MobaXterm, WindTerm et les définitions JSON de NyaTerm
- Import/export chiffré de l'ensemble de l'application au format `.nya` pour une configuration NyaTerm portable

---

<a name="screenshots"></a>
# Captures d'écran

## Espace de travail

Gérez vos sessions SSH, shell local, Telnet et série dans un même espace de travail à onglets et volets divisés.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/overview-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/overview-light.png">
    <img alt="Vue d'ensemble de l'espace de travail de NyaTerm" src="../static/img/home/overview-light.png">
  </picture>
</p>

## Apparence et image de fond

Utilisez une image locale comme fond de la fenêtre principale, réglez `Image Sizing`, `Image Opacity` et `Background Content Opacity`, et conservez la lisibilité des fenêtres secondaires avec des fonds unis.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/cover-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/cover-light.png">
    <img alt="Personnalisation de l'image de fond de NyaTerm" src="../static/img/home/cover-light.png">
  </picture>
</p>

## Améliorations du terminal

Utilisez l'historique des commandes, la recherche, la traduction, les liens d'action, les horodatages, le surlignage des mots-clés et la protection contre les sorties volumineuses dans le terminal.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/terminal-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/terminal-light.png">
    <img alt="Fonctionnalités du terminal de NyaTerm" src="../static/img/home/terminal-light.png">
  </picture>
</p>

## Fichiers distants

Parcourez les fichiers SFTP à côté du terminal, gérez les transferts et renvoyez les modifications de votre éditeur local vers le chemin distant.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/files-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/files-light.png">
    <img alt="Gestion des fichiers SFTP dans NyaTerm" src="../static/img/home/files-light.png">
  </picture>
</p>

## Outils de sécurité et de réseau

Gérez identifiants, OTP, hôtes connus, proxys, hôtes de rebond et tunnels SSH depuis des panneaux dédiés.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/security-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/security-light.png">
    <img alt="Outils de sécurité et de réseau de NyaTerm" src="../static/img/home/security-light.png">
  </picture>
</p>

## Synchronisation et sauvegarde

Synchronisez des instantanés chiffrés de configuration portable et restaurez vos sauvegardes via WebDAV ou un stockage compatible S3.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../static/img/home/sync-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="../static/img/home/sync-light.png">
    <img alt="Synchronisation et sauvegarde de NyaTerm" src="../static/img/home/sync-light.png">
  </picture>
</p>

---

<a name="supported-platforms"></a>
# Plateformes prises en charge

| Système | Prise en charge |
| :--- | :--- |
| **Windows** | Windows 10/11, x64 / arm64 |
| **macOS** | macOS 12+, Intel / Apple Silicon |
| **Linux** | Ubuntu 20.04+, Fedora 36+, Arch Linux et distributions similaires |

Pour les bureaux Ubuntu 20.04 x86-64 personnalisés utilisant **glibc 2.31**, utilisez la
[version de compatibilité hors ligne focal](../../scripts/compat/linux-focal/README.md).
Les paquets Linux standard sont compilés sur des distributions plus récentes et peuvent nécessiter
une version plus récente de glibc. La version de compatibilité comprend tous les processus auxiliaires d'exécution, un rapport ABI,
un lanceur privilégiant le système et le GPU, ainsi qu'un repli OpenGL sur CPU dédié, à activer explicitement ;
la machine cible doit disposer d'une session X11 fonctionnelle.

Téléchargez les programmes d'installation sur [nyaterm.app](https://nyaterm.app) ou sur la page [Releases](https://github.com/nyakang/nyaterm/releases).

---

<a name="supported-session-types"></a>
# Types de sessions pris en charge

| Type | Usage courant | Remarques |
|------|-------------|-------|
| SSH | Serveurs distants Linux / Unix | Prend en charge SFTP, OTP, le suivi des ressources / GPU / processus / Docker, les proxys, hôtes de rebond, tunnels et préférences d'algorithmes par connexion |
| Terminal local | Travail dans un shell local | Utilise le chemin du shell et le répertoire de travail locaux |
| Telnet | Anciens équipements réseau ou systèmes de laboratoire | Session de terminal légère sans les fonctions propres à SSH, avec `Backspace Mode` pour `Ctrl+H (BS)` ou `DEL (0x7F)` |
| Série | Routeurs, cartes et appareils embarqués | Port, débit en bauds, bits de données, parité, bits d'arrêt et `Backspace Mode` configurables |

---

<a name="getting-started"></a>
# Premiers pas

## Téléchargement

Téléchargez la dernière version pour votre plateforme sur [nyaterm.app](https://nyaterm.app) ou [Releases](https://github.com/nyakang/nyaterm/releases).

| Plateforme | Format |
|----------|--------|
| Windows | `.exe` / `.zip` portable |
| macOS | `.dmg` |
| Linux | `.deb` / `.AppImage` / `.rpm` |

Pour l'édition portable de Windows, extrayez le ZIP et lancez `NyaTerm.exe` ; les données sont conservées dans le répertoire `data/` voisin. L'action GPUI **Aide → Rechercher des mises à jour** utilise des manifestes signés et permet de télécharger et d'installer les mises à jour pour les types d'installation pris en charge. Les éditions Linux deb/rpm installent les mises à jour signées via le gestionnaire de paquets du système, avec autorisation Polkit ou sudo, puis proposent un redémarrage. Les dépendances doivent déjà être installées. Les versions Homebrew et celles gérées par des outils externes utilisent leur gestionnaire de paquets.

**Migration depuis la version 1.x : les éditions portables Windows et les anciennes installations MSI nécessitent une mise à niveau manuelle.** Quittez d'abord l'application et sauvegardez toutes les données et clés. Remplacez l'ensemble des fichiers du programme portable en conservant `data/` ; pour MSI, sauvegardez, désinstallez l'ancienne version et installez la version NSIS. Remplacer uniquement l'exécutable principal ne suffit pas, et la migration automatique n'est pas garantie pour tous les anciens installateurs. Consultez le [guide de migration vers la version 2.0](https://nyaterm.app/en/docs/getting-started/migration-v2).

### macOS

Sous macOS, vous pouvez installer NyaTerm avec Homebrew :

```bash
brew install nyakang/nyaterm/nyaterm
```

Cette commande utilise le tap [`nyakang/homebrew-nyaterm`](https://github.com/nyakang/homebrew-nyaterm) et installe le cask `nyaterm`. Vous pouvez aussi télécharger le programme d'installation `.dmg` sur [nyaterm.app](https://nyaterm.app) ou [Releases](https://github.com/nyakang/nyaterm/releases), puis glisser NyaTerm dans `/Applications`.

NyaTerm n'est actuellement pas signé avec un certificat Apple Developer. Si macOS indique après l'installation que l'application est endommagée ou ne peut pas être ouverte, supprimez l'attribut de quarantaine puis relancez-la :

```bash
sudo xattr -cr /Applications/NyaTerm.app
```

### Arch Linux / AUR

Sous Arch Linux, vous pouvez installer NyaTerm depuis l'AUR :

```bash
yay -S nyaterm-bin
```

Ou avec `paru` :

```bash
paru -S nyaterm-bin
```

Vous pouvez également le compiler manuellement depuis le dépôt AUR :

```bash
git clone https://aur.archlinux.org/nyaterm-bin.git
cd nyaterm-bin
makepkg -si
```

Paquet AUR : [`nyaterm-bin`](https://aur.archlinux.org/packages/nyaterm-bin)

> `nyaterm-bin` est un paquet binaire AUR maintenu par la communauté. Si le paquet AUR n'a pas encore été mis à jour vers la dernière version, téléchargez le paquet officiel depuis [Releases](https://github.com/nyakang/nyaterm/releases).


## Prérequis de développement

- Rust stable installé via [rustup](https://rustup.rs/)
- Dépendances de plateforme requises par GPUI et votre système d'exploitation

## Développement

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
cargo run -p nyaterm-app --bin nyaterm
```

## Structure du projet

```text
├── crates/
│   ├── nyaterm-app/             # Point d'entrée et ressources de l'application native GPUI
│   ├── nyaterm-desktop/         # Composition de l'application GPUI et état des fonctionnalités
│   ├── nyaterm-core/            # Modèles métier et politiques indépendants de l'interface
│   ├── nyaterm-terminal/        # Machine à états et analyse du terminal
│   ├── nyaterm-terminal-runtime/ # Traitement des trames, recherche et routage des événements de session
│   ├── nyaterm-terminal-gpui/   # Disposition, saisie et rendu du terminal GPUI
│   ├── nyaterm-transport/       # SSH, PTY, SFTP, Telnet, série et transferts
│   ├── nyaterm-store/           # Persistance et compatibilité de la base de données
│   ├── nyaterm-ui/              # Contrôles GPUI partagés et intégration des thèmes
│   ├── nyaterm-remote-desktop/  # Sessions RDP/VNC indépendantes de l'interface et IPC typées
│   ├── nyaterm-rdp-helper/      # Processus auxiliaire isolé du protocole IronRDP
│   ├── nyaterm-vnc-helper/      # Processus auxiliaire isolé du protocole VNC
│   └── nyaterm-otp/             # Implémentation compatible OTP
├── docs-site/                   # Site de documentation Docusaurus
├── Cargo.toml                   # Définition de l'espace de travail Rust
└── Cargo.lock
```

---

<a name="credits"></a>
# Remerciements

Merci aux projets et bibliothèques suivants qui rendent NyaTerm possible :

- [WindTerm](https://github.com/kingToolbox/WindTerm) — a inspiré la conception et les fonctionnalités de NyaTerm
- [tabby](https://github.com/Eugeny/tabby) — un excellent terminal multiplateforme qui a inspiré de nombreux choix de conception
- [Alacritty](https://github.com/alacritty/alacritty) — composants d'émulation de terminal utilisés par NyaTerm
- [russh](https://github.com/warp-tech/russh) — une implémentation de client et serveur SSH en Rust

---

<a name="sponsor"></a>
# Soutenir le projet

Si NyaTerm vous aide au quotidien dans le développement, l'administration ou le travail sur terminal distant, vous pouvez soutenir sa maintenance depuis la [page de soutien](./en/docusaurus-plugin-content-docs/current/sponsor.md).

---

<a name="contributors"></a>
# Contributeurs

<a href="https://github.com/nyakang/nyaterm/graphs/contributors">
  <img src="https://contrib-wall.coderkang.workers.dev/image?snapshot=EEZNlyhjBOrARLki-MUhXqKk&amp;sealed_token=eyJ2IjoxLCJ0eXBlIjoic25hcHNob3QiLCJzbmFwc2hvdCI6IkVFWk5seWhqQk9yQVJMa2ktTVVoWHFLayIsImV4cCI6MTgxNTAzNjQ0NH0.UwegiwfiT3KSKQ17n5ivt24EFwAtoaHZGRwx97t0ODA&amp;sealed_github_token=A6pS93BW-53hz_Cs.8rPY0yM6koNS_UowTbu8qkWzmPLRco8HJRJO2_yp5MZH9yz9o1Ob2g5MUL4aLFK24gjbDexjdErw6eR72ruRbNcsFciYuM4DghLZJgi9V5D9f1-_xFT-sGYK6ibc1A4aWz8EyvFIQo0byPAXv-d410cJmhwsH-bqc963L8J-z0kU6oIi-2zCSszxllze23WmJsWnTJBFUBrQbllg5cLwuHm6q-BMkjoVxChLdOMVwEs" alt="Contributeurs" />
</a>

---

## Historique des étoiles

<a href="https://www.star-history.com/?repos=nyakang%2Fnyaterm&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&theme=dark&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
   <img alt="Graphique de l'historique des étoiles" src="https://api.star-history.com/chart?repos=nyakang/nyaterm&type=date&legend=top-left&sealed_token=B9FcRzP_4KoFVgTzgY_L9J4F1huRnhx9N962VMQBHjFsF-VtiApJNnKYWg5IaJDZtDm7iCd3epIU3uEZTSG2XVoU8jVssH-ge-gPjPGq5cRz9xtF2N_piA" />
 </picture>
</a>

---

<a name="license"></a>
# Licence

Ce projet est distribué sous [licence Apache 2.0](../../LICENSE).
