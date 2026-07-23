<script type="text/x-dc" data-dc-script data-props="{&quot;$preview&quot;:{&quot;width&quot;:&quot;100%&quot;,&quot;height&quot;:&quot;100%&quot;},&quot;startView&quot;:{&quot;editor&quot;:&quot;enum&quot;,&quot;options&quot;:[&quot;landing&quot;,&quot;login&quot;,&quot;app&quot;],&quot;default&quot;:&quot;landing&quot;,&quot;tsType&quot;:&quot;'landing'|'login'|'app'&quot;,&quot;section&quot;:&quot;Preview&quot;},&quot;defaultLang&quot;:{&quot;editor&quot;:&quot;enum&quot;,&quot;options&quot;:[&quot;pt&quot;,&quot;en&quot;],&quot;default&quot;:&quot;pt&quot;,&quot;tsType&quot;:&quot;'pt'|'en'&quot;,&quot;section&quot;:&quot;Preview&quot;},&quot;panelTheme&quot;:{&quot;editor&quot;:&quot;enum&quot;,&quot;options&quot;:[&quot;dark&quot;,&quot;light&quot;],&quot;default&quot;:&quot;dark&quot;,&quot;tsType&quot;:&quot;'dark'|'light'&quot;,&quot;section&quot;:&quot;Preview&quot;}}">
class Component extends DCLogic {
  state = {
    lang: this.props.defaultLang || 'pt', theme: this.props.panelTheme || 'dark', view: this.props.startView || 'landing', tab: 'links',
    createOpen: false, copied: '', genInput: '', genCode: '', genBusy: false, genDone: false, genTargetShown: '', statLink: 'summer24'
  };

  strings() {
    const pt = {
      nav: { features: 'Recursos', pricing: 'Preços', how: 'Como funciona', langLabel: 'PT / EN', signin: 'Entrar', cta: 'Começar' },
      hero: {
        eyebrow: 'ENCURTADOR DE LINKS',
        title1: 'Links curtos com a sua ', title2: 'marca', title3: ', e métricas de verdade.',
        sub: 'Crie links curtos, use seu próprio domínio, gere QR codes e acompanhe cada clique em tempo real. Grátis para hospedar você mesmo, ou use a nuvem pronta com planos.',
        genPlaceholder: 'Cole um link longo…',
        genBtn: 'Encurtar',
        genNote: 'Demo. O código é uma bijeção com chave (Feistel/ARX). Crie uma conta para salvar e medir cliques.',
      },
      heroStats: [
        { value: 'Seu domínio', label: 'no lugar de q.rk' },
        { value: 'Cada clique', label: 'medido em tempo real' },
        { value: 'Open-source', label: 'ou nuvem pronta' },
      ],
      how: {
        label: 'COMO FUNCIONA',
        title: 'O código é a permutação do id.',
        lead: 'Sem tabela de string para id, sem checagem de colisão. Uma bijeção com chave transforma o id inteiro em um código de 7 caracteres, e de volta, em pura aritmética.',
        steps: [
          { k: '01', t: 'Cole o link', d: 'Um POST e o quark aloca o próximo id interno.' },
          { k: '02', t: 'Calcula, não guarda', d: 'O id vira código por uma permutação Feistel/ARX reversível.' },
          { k: '03', t: 'Redireciona em ~2 ms', d: 'Decodifica o código, checa o cache, responde 302.' },
        ],
        termTitle: 'criar-link.sh',
        termOut: 'code gerado por aritmética, sem lookup',
      },
      features: {
        label: 'RECURSOS',
        title: 'Tudo que o seu marketing precisa de um link.',
        sub: 'De QR codes a analytics em tempo real. Fácil no dia a dia, poderoso quando você precisa.',
        items: [
          { t: 'Analytics de cliques', d: 'Cliques por dia, país, dispositivo, referer e navegador. Bots filtrados, sem cookies invasivos.' },
          { t: 'QR codes, tags e pastas', d: 'QR para cada link, organização por tags e pastas, busca instantânea server-side.' },
          { t: 'Regras geo & dispositivo', d: 'Mande iOS pra App Store, Android pro Google Play e o resto pro site.' },
          { t: 'Testes A/B', d: 'Variantes com peso e estatística por variante, direto no link.' },
          { t: 'Senha & expiração', d: 'Proteja com senha (argon2), expire por data ou por número de visitas.' },
          { t: 'Webhooks & API tokens', d: 'Eventos assinados pra Zapier, Make e Slack; tokens com escopo por permissão.' },
        ],
      },
      audience: {
        label: 'PARA QUEM É',
        title: 'Feito para os dois lados.',
        growth: { tag: 'Times de growth', desc: 'Marketing, social e vendas.', feats: ['Domínio próprio com SSL', 'Modelos de UTM salvos', 'QR code em cada link', 'Testes A/B com estatística', 'Cliques por país e dispositivo', 'Sem cookies invasivos'] },
        dev: { tag: 'Desenvolvedores', desc: 'Para hospedar e integrar.', feats: ['Open-source (AGPL-3.0)', 'Binário único de ~1 MB, sem dependências', 'API HTTP com tokens por escopo', 'Webhooks assinados', 'Suba com um comando'] },
      },
      engine: {
        label: 'POR BAIXO DO CAPÔ',
        title: 'Rápido porque é simples.',
        lead: 'O código curto é calculado, não guardado num banco: uma função com chave gera o código e o resolve de volta. Na prática, isso vira redirecionamentos em poucos milissegundos, sem banco no caminho.',
        link: 'Ver os benchmarks no GitHub',
        stats: [
          { n: '~2 ms', l: 'no redirecionamento' },
          { n: '~1 MB', l: 'binário único' },
          { n: '0', l: 'dependências' },
          { n: '0', l: 'erros em 225k requisições' },
        ],
      },
      bench: {
        label: 'BENCHMARKS',
        title: 'Rápido o bastante pra nunca ser o gargalo.',
        cards: [
          { n: '~22M/s', l: 'códigos por segundo', c: '#C6F94E' },
          { n: '18×', l: 'mais rápido que Feistel+HMAC', c: '#4ADEDE' },
          { n: '4', l: 'rounds: difusão medida, não chutada', c: '#8B7CF6' },
          { n: '0', l: 'erros em ~225k requisições', c: '#C6F94E' },
        ],
        note: 'A não-enumerabilidade é uma propriedade estatística medida (avalanche/SAC), não garantia criptográfica. Números medidos com criterion e k6. Reproduza com cargo bench.',
      },
      pricing: {
        label: 'PREÇOS',
        title: 'Comece de graça. Escale quando quiser.',
        sub: 'Self-hosted é livre e sem limites. A nuvem cuida da infra por você.',
        per: '/ mês',
        plans: [
          { name: 'Self-hosted', price: 'Grátis', unit: 'para sempre', desc: 'Um binário. Você hospeda.', cta: 'Ver no GitHub', hot: false, feats: ['Código AGPL-3.0 aberto', 'Binário único, zero dependências', 'Links, analytics e QR ilimitados', 'Postgres / Valkey / ClickHouse opcionais', 'Suporte da comunidade'] },
          { name: 'Cloud Pro', price: 'R$ 39', unit: '/ mês', desc: 'Pra criadores e times pequenos.', cta: 'Começar', hot: true, feats: ['Tudo do self-hosted, gerenciado', 'Domínio próprio + SSL', '5 membros na workspace', 'Analytics com retenção de 1 ano', 'Webhooks e API tokens'] },
          { name: 'Cloud Business', price: 'R$ 149', unit: '/ mês', desc: 'Pra empresas e escala.', cta: 'Falar com vendas', hot: false, feats: ['Membros ilimitados + SSO/OIDC', 'Múltiplos domínios e workspaces', 'Retenção estendida + exportação', 'SLA e suporte prioritário', 'Isolamento por tenant'] },
        ],
        free: 'Também há um Cloud Free pra testar sem cartão.',
      },
      cta: {
        title: 'Comece de graça hoje.',
        sub: 'Crie sua conta na nuvem em segundos. Sem cartão.',
        btn: 'Criar conta grátis',
        btn2: 'Ver no GitHub',
        devNote: 'Prefere hospedar você mesmo?',
      },
      footer: { tag: 'O código é matemática, não uma linha no banco.', made: 'Feito com Rust', links: ['Recursos', 'Preços', 'Docs', 'GitHub'], license: 'AGPL-3.0 · © 2026 Lucas Olopes' },
      login: { eyebrow: 'PAINEL', title: 'Entre na sua workspace', sub: 'Acesse seus links, analytics e configurações.', email: 'E-mail', emailPh: 'voce@empresa.com', password: 'Senha', passwordPh: '••••••••', forgot: 'Esqueceu?', submit: 'Continuar', or: 'ou', google: 'Entrar com Google', token: 'Usar token de admin (self-hosted)', tokenPh: 'QUARK_ADMIN_TOKEN', back: '← Voltar ao site', noAccount: 'Não tem conta?', signup: 'Criar grátis', terms: 'Ao continuar, você concorda com os Termos e a Privacidade.' },
      panel: {
        nav: { links: 'Links', analytics: 'Analytics', domains: 'Domínios', members: 'Membros', tokens: 'API Tokens', webhooks: 'Webhooks' },
        groupMain: 'Workspace', groupSettings: 'Configurações',
        search: 'Buscar links…', newLink: 'Novo link', plan: 'Cloud Pro', upgrade: 'Fazer upgrade',
        links: { title: 'Links', allFolders: 'Todos', clicks: 'cliques', copy: 'Copiar', copied: 'Copiado', created: 'criado', qr: 'QR code', stats: 'Analytics', edit: 'Editar', del: 'Excluir', tagFilter: 'Tags' },
        create: { title: 'Criar link curto', dest: 'URL de destino', destPh: 'https://exemplo.com/pagina', alias: 'Alias personalizado', aliasPh: 'promo-verao', folder: 'Pasta', noFolder: 'Sem pasta', tags: 'Tags', tagsPh: 'adicionar tag e Enter', ttl: 'Expira em', utmToggle: 'Parâmetros UTM', src: 'utm_source', med: 'utm_medium', camp: 'utm_campaign', preview: 'Seu link ficará assim', cancel: 'Cancelar', submit: 'Criar link' },
        an: { title: 'Analytics do link', back: '← Voltar aos links', range: 'Últimos 30 dias', total: 'Cliques totais', unique: 'Únicos', topGeo: 'Top país', topDev: 'Top dispositivo', perDay: 'Cliques por dia', byCountry: 'Por país', byDevice: 'Por dispositivo', byBrowser: 'Por navegador', byRef: 'Por origem', recent: 'Cliques recentes', colWhen: 'Quando', colGeo: 'Local', colDev: 'Dispositivo', colRef: 'Origem', bots: 'bots filtrados' },
        dom: { title: 'Domínios', sub: 'Use seu próprio domínio nos links curtos.', add: 'Adicionar domínio', active: 'Ativo', pending: 'DNS pendente', verify: 'Verificar', links: 'links', primary: 'Primário' },
        mem: { title: 'Membros', sub: 'Quem tem acesso a esta workspace.', invite: 'Convidar membro', owner: 'Dono', admin: 'Admin', member: 'Membro', pending: 'Convite pendente', seats: 'assentos usados' },
        tok: { title: 'API Tokens', sub: 'Tokens com escopo por permissão para a API HTTP.', create: 'Criar token', scopes: 'Escopos', rate: 'Limite', lastUsed: 'Último uso', revoke: 'Revogar', never: 'nunca' },
        wh: { title: 'Webhooks', sub: 'Eventos assinados enviados para seus endpoints.', add: 'Adicionar webhook', active: 'Ativo', paused: 'Pausado', test: 'Testar', events: 'eventos', delivered: 'entregue' },
      },
    };
    const en = {
      nav: { features: 'Features', pricing: 'Pricing', how: 'How it works', langLabel: 'EN / PT', signin: 'Sign in', cta: 'Get started' },
      hero: {
        eyebrow: 'URL SHORTENER',
        title1: 'Short links with your ', title2: 'brand', title3: ', and analytics you can trust.',
        sub: 'Create short links, use your own domain, generate QR codes and track every click in real time. Free to self-host, or use the hosted cloud with plans.',
        genPlaceholder: 'Paste a long link…',
        genBtn: 'Shorten',
        genNote: 'Demo. The code is a keyed bijection (Feistel/ARX). Create an account to save links and measure clicks.',
      },
      heroStats: [
        { value: 'Your domain', label: 'instead of q.rk' },
        { value: 'Every click', label: 'measured in real time' },
        { value: 'Open-source', label: 'or hosted cloud' },
      ],
      how: {
        label: 'HOW IT WORKS',
        title: 'The code is the permutation of the id.',
        lead: 'No string-to-id table, no collision checks. A keyed bijection turns the integer id into a 7-character code, and back, in pure arithmetic.',
        steps: [
          { k: '01', t: 'Paste the link', d: 'One POST and quark allocates the next internal id.' },
          { k: '02', t: 'Compute, don\u2019t store', d: 'The id becomes a code via a reversible Feistel/ARX permutation.' },
          { k: '03', t: 'Redirect in ~2 ms', d: 'Decode the code, check the cache, answer 302.' },
        ],
        termTitle: 'create-link.sh',
        termOut: 'code produced by arithmetic, no lookup',
      },
      features: {
        label: 'FEATURES',
        title: 'Everything your marketing needs from a link.',
        sub: 'From QR codes to real-time analytics. Easy day to day, powerful when you need it.',
        items: [
          { t: 'Click analytics', d: 'Clicks by day, country, device, referer and browser. Bots filtered, no invasive cookies.' },
          { t: 'QR codes, tags & folders', d: 'A QR per link, organize with tags and folders, instant server-side search.' },
          { t: 'Geo & device rules', d: 'Send iOS to the App Store, Android to Google Play, everyone else to the site.' },
          { t: 'A/B testing', d: 'Weighted variants with per-variant stats, right on the link.' },
          { t: 'Password & expiry', d: 'Protect with a password (argon2), expire by date or by number of visits.' },
          { t: 'Webhooks & API tokens', d: 'Signed events to Zapier, Make and Slack; tokens scoped per permission.' },
        ],
      },
      audience: {
        label: 'WHO IT IS FOR',
        title: 'Built for both sides.',
        growth: { tag: 'Growth teams', desc: 'Marketing, social and sales.', feats: ['Custom domain with SSL', 'Saved UTM templates', 'A QR code on every link', 'A/B tests with stats', 'Clicks by country and device', 'No invasive cookies'] },
        dev: { tag: 'Developers', desc: 'To self-host and integrate.', feats: ['Open-source (AGPL-3.0)', 'Single ~1 MB binary, no dependencies', 'HTTP API with scoped tokens', 'Signed webhooks', 'Ship it in one command'] },
      },
      engine: {
        label: 'UNDER THE HOOD',
        title: 'Fast because it is simple.',
        lead: 'The short code is computed, not stored in a database: a keyed function generates the code and resolves it back. In practice that means redirects in a few milliseconds, with no database in the path.',
        link: 'See the benchmarks on GitHub',
        stats: [
          { n: '~2 ms', l: 'per redirect' },
          { n: '~1 MB', l: 'single binary' },
          { n: '0', l: 'dependencies' },
          { n: '0', l: 'errors across 225k requests' },
        ],
      },
      bench: {
        label: 'BENCHMARKS',
        title: 'Fast enough to never be the bottleneck.',
        cards: [
          { n: '~22M/s', l: 'codes per second', c: '#C6F94E' },
          { n: '18\u00d7', l: 'faster than Feistel+HMAC', c: '#4ADEDE' },
          { n: '4', l: 'rounds: diffusion measured, not guessed', c: '#8B7CF6' },
          { n: '0', l: 'errors across ~225k requests', c: '#C6F94E' },
        ],
        note: 'Non-enumerability is a measured statistical property (avalanche/SAC), not a cryptographic guarantee. Numbers measured with criterion and k6. Reproduce with cargo bench.',
      },
      pricing: {
        label: 'PRICING',
        title: 'Start free. Scale when you want.',
        sub: 'Self-hosted is free and unlimited. The cloud handles the infra for you.',
        per: '/ mo',
        plans: [
          { name: 'Self-hosted', price: 'Free', unit: 'forever', desc: 'One binary. You host it.', cta: 'View on GitHub', hot: false, feats: ['Open AGPL-3.0 source', 'Single binary, zero dependencies', 'Unlimited links, analytics and QR', 'Optional Postgres / Valkey / ClickHouse', 'Community support'] },
          { name: 'Cloud Pro', price: '$9', unit: '/ mo', desc: 'For creators and small teams.', cta: 'Get started', hot: true, feats: ['Everything self-hosted, managed', 'Custom domain + SSL', '5 workspace members', 'Analytics with 1-year retention', 'Webhooks and API tokens'] },
          { name: 'Cloud Business', price: '$29', unit: '/ mo', desc: 'For companies and scale.', cta: 'Talk to sales', hot: false, feats: ['Unlimited members + SSO/OIDC', 'Multiple domains and workspaces', 'Extended retention + export', 'SLA and priority support', 'Per-tenant isolation'] },
        ],
        free: 'There is also a Cloud Free to try without a card.',
      },
      cta: {
        title: 'Start free today.',
        sub: 'Create your cloud account in seconds. No card.',
        btn: 'Create free account',
        btn2: 'View on GitHub',
        devNote: 'Prefer to self-host?',
      },
      footer: { tag: 'The code is math, not a row in a database.', made: 'Made with Rust', links: ['Features', 'Pricing', 'Docs', 'GitHub'], license: 'AGPL-3.0 · © 2026 Lucas Olopes' },
      login: { eyebrow: 'PANEL', title: 'Sign in to your workspace', sub: 'Access your links, analytics and settings.', email: 'Email', emailPh: 'you@company.com', password: 'Password', passwordPh: '••••••••', forgot: 'Forgot?', submit: 'Continue', or: 'or', google: 'Sign in with Google', token: 'Use admin token (self-hosted)', tokenPh: 'QUARK_ADMIN_TOKEN', back: '← Back to site', noAccount: 'No account?', signup: 'Create free', terms: 'By continuing you agree to the Terms and Privacy.' },
      panel: {
        nav: { links: 'Links', analytics: 'Analytics', domains: 'Domains', members: 'Members', tokens: 'API Tokens', webhooks: 'Webhooks' },
        groupMain: 'Workspace', groupSettings: 'Settings',
        search: 'Search links…', newLink: 'New link', plan: 'Cloud Pro', upgrade: 'Upgrade',
        links: { title: 'Links', allFolders: 'All', clicks: 'clicks', copy: 'Copy', copied: 'Copied', created: 'created', qr: 'QR code', stats: 'Analytics', edit: 'Edit', del: 'Delete', tagFilter: 'Tags' },
        create: { title: 'Create short link', dest: 'Destination URL', destPh: 'https://example.com/page', alias: 'Custom alias', aliasPh: 'summer-promo', folder: 'Folder', noFolder: 'No folder', tags: 'Tags', tagsPh: 'add tag and Enter', ttl: 'Expires in', utmToggle: 'UTM parameters', src: 'utm_source', med: 'utm_medium', camp: 'utm_campaign', preview: 'Your link will look like', cancel: 'Cancel', submit: 'Create link' },
        an: { title: 'Link analytics', back: '← Back to links', range: 'Last 30 days', total: 'Total clicks', unique: 'Unique', topGeo: 'Top country', topDev: 'Top device', perDay: 'Clicks per day', byCountry: 'By country', byDevice: 'By device', byBrowser: 'By browser', byRef: 'By referrer', recent: 'Recent clicks', colWhen: 'When', colGeo: 'Location', colDev: 'Device', colRef: 'Referrer', bots: 'bots filtered' },
        dom: { title: 'Domains', sub: 'Use your own domain on short links.', add: 'Add domain', active: 'Active', pending: 'DNS pending', verify: 'Verify', links: 'links', primary: 'Primary' },
        mem: { title: 'Members', sub: 'Who has access to this workspace.', invite: 'Invite member', owner: 'Owner', admin: 'Admin', member: 'Member', pending: 'Pending invite', seats: 'seats used' },
        tok: { title: 'API Tokens', sub: 'Permission-scoped tokens for the HTTP API.', create: 'Create token', scopes: 'Scopes', rate: 'Rate', lastUsed: 'Last used', revoke: 'Revoke', never: 'never' },
        wh: { title: 'Webhooks', sub: 'Signed events delivered to your endpoints.', add: 'Add webhook', active: 'Active', paused: 'Paused', test: 'Test', events: 'events', delivered: 'delivered' },
      },
    };
    return this.state.lang === 'pt' ? pt : en;
  }

  genCodeStr() {
    const chars = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';
    let s = '';
    for (let i = 0; i < 7; i++) s += chars[Math.floor(Math.random() * chars.length)];
    return s;
  }

  shorten() {
    const raw = (this.state.genInput || '').trim() || 'https://example.com/uma/url/muito/comprida';
    const target = /^https?:\/\//.test(raw) ? raw : 'https://' + raw;
    this.setState({ genBusy: true, genDone: false });
    setTimeout(() => {
      this.setState({ genBusy: false, genDone: true, genCode: this.genCodeStr(), genTargetShown: target, copied: '' });
    }, 520);
  }

  copy(text, id) {
    try { navigator.clipboard && navigator.clipboard.writeText(text); } catch (e) {}
    this.setState({ copied: id });
    setTimeout(() => { if (this.state.copied === id) this.setState({ copied: '' }); }, 1600);
  }

  palette() {
    if (this.state.theme === 'light') {
      return { bg: '#EEF0F4', ink2: '#E7EAF0', card: '#FFFFFF', card2: '#F4F5F8', text: '#12141C', strong: '#12141C', muted: '#5E6472', border: 'rgba(0,0,0,0.10)', borderStrong: 'rgba(0,0,0,0.16)', hover: 'rgba(0,0,0,0.04)', input: '#FFFFFF', brand: '#4A7A17', fill: '#8FD12E', wash: 'rgba(143,209,46,.14)', shadow: '0 1px 2px rgba(0,0,0,.08)' };
    }
    return { bg: '#0A0B0F', ink2: '#0C0D13', card: '#131521', card2: '#1A1D2B', text: '#E8EAF2', strong: '#F3F5FA', muted: '#8A90A2', border: 'rgba(255,255,255,0.09)', borderStrong: 'rgba(255,255,255,0.16)', hover: 'rgba(255,255,255,0.04)', input: '#0F1119', brand: '#C6F94E', fill: '#C6F94E', wash: 'rgba(198,249,78,.12)', shadow: '0 1px 2px rgba(0,0,0,.4)' };
  }

  fmt(n) { return n.toLocaleString(this.state.lang === 'pt' ? 'pt-BR' : 'en-US'); }

  data() {
    const links = [
      { code: 'aZ3k9Qp', alias: false, dest: 'https://acme.com/black-friday-2026/ofertas-especiais', clicks: 48213, tags: ['campanha', 'vendas'], folder: 'Marketing', created: '2d', ttl: '—' },
      { code: 'summer24', alias: true, dest: 'https://acme.com/verao/landing-page', clicks: 29845, tags: ['campanha'], folder: 'Marketing', created: '5d', ttl: '30d' },
      { code: 'Kp72mNx', alias: false, dest: 'https://blog.acme.com/encurtar-urls-com-rust', clicks: 12094, tags: ['blog'], folder: 'Conteúdo', created: '1sem', ttl: '—' },
      { code: 'docs-api', alias: true, dest: 'https://docs.acme.com/api/reference', clicks: 8340, tags: ['docs'], folder: 'Conteúdo', created: '2sem', ttl: '—' },
      { code: '9wQ2bLz', alias: false, dest: 'https://acme.com/webinar/inscricao', clicks: 5127, tags: ['evento', 'campanha'], folder: 'Marketing', created: '3sem', ttl: '7d' },
      { code: 'app-ios', alias: true, dest: 'https://apps.apple.com/br/app/acme', clicks: 3908, tags: ['app'], folder: '—', created: '1mês', ttl: '—' },
      { code: 'Xy4Rt8v', alias: false, dest: 'https://acme.com/newsletter/assinar', clicks: 1622, tags: ['email'], folder: '—', created: '1mês', ttl: '—' },
    ];
    const tagColors = { campanha: '#C6F94E', vendas: '#4ADEDE', blog: '#8B7CF6', docs: '#4ADEDE', evento: '#FEBC2E', app: '#8B7CF6', email: '#4ADEDE' };
    const domains = [
      { host: 'acme.link', status: 'active', links: 38, primary: true },
      { host: 'go.acme.com', status: 'active', links: 12, primary: false },
      { host: 'try.acme.io', status: 'pending', links: 0, primary: false },
    ];
    const members = [
      { name: 'Lucas Olopes', email: 'lucas@acme.com', role: 'owner', initials: 'LO', color: '#C6F94E' },
      { name: 'Marina Reis', email: 'marina@acme.com', role: 'admin', initials: 'MR', color: '#4ADEDE' },
      { name: 'Diego Santos', email: 'diego@acme.com', role: 'member', initials: 'DS', color: '#8B7CF6' },
      { name: 'ana@partner.com', email: '', role: 'pending', initials: 'A', color: '#8A90A2' },
    ];
    const tokens = [
      { name: 'CI de produção', prefix: 'qtok_live_a1e4', scopes: ['links_write', 'analytics'], rate: '600/min', lastUsed: '2h' },
      { name: 'Zapier', prefix: 'qtok_live_9f0c', scopes: ['links_write'], rate: '120/min', lastUsed: '1d' },
      { name: 'Dashboard read-only', prefix: 'qtok_live_c7b2', scopes: ['links_read', 'analytics'], rate: '300/min', lastUsed: 'never' },
    ];
    const webhooks = [
      { url: 'https://hooks.zapier.com/hooks/catch/8842/a1b2c3', kind: 'Zapier', evs: ['link.created', 'link.clicked'], status: 'active', del: '99.8%' },
      { url: 'https://acme.com/api/quark-events', kind: 'generic', evs: ['link.created', 'link.updated', 'link.deleted'], status: 'active', del: '100%' },
      { url: 'https://hooks.slack.com/services/T04/B07/xY9', kind: 'Slack', evs: ['link.threshold_reached'], status: 'paused', del: '—' },
    ];
    const perDay = [820, 910, 1180, 1040, 1330, 1610, 1490, 1720, 2010, 1880, 2240, 2680, 2510, 2960];
    return { links, tagColors, domains, members, tokens, webhooks, perDay };
  }

  renderVals() {
    const t = this.strings();
    const v = this.state.view;
    const genShort = 'q.rk/' + (this.state.genCode || 'aZ3k9Qp');
    const target = this.state.genTargetShown;
    const cp = this.state.lang === 'pt' ? { copy: 'Copiar', copied: 'Copiado!' } : { copy: 'Copy', copied: 'Copied!' };
    return {
      lang: this.state.lang,
      isLanding: v === 'landing', isLogin: v === 'login', isApp: v === 'app',
      t,
      heroStats: t.heroStats,
      howSteps: t.how.steps,
      features: t.features.items,
      growthFeats: t.audience.growth.feats,
      devFeats: t.audience.dev.feats,
      engineStats: t.engine.stats,
      benchCards: t.bench.cards,
      plans: t.pricing.plans.map(p => ({
        ...p,
        cardBg: p.hot ? '#141726' : '#131521',
        cardBorder: p.hot ? 'rgba(198,249,78,.45)' : 'rgba(255,255,255,.09)',
        cardShadow: p.hot ? '0 24px 60px -34px rgba(198,249,78,.35)' : 'none',
        btnBg: p.hot ? '#C6F94E' : 'transparent',
        btnColor: p.hot ? '#0A0B0F' : '#E8EAF2',
        btnBorder: p.hot ? '#C6F94E' : 'rgba(255,255,255,.18)',
      })),
      onCopyCmd: () => this.copy('docker run -p 8080:8080 quark', 'cmd'),
      cmdCopyLabel: this.state.copied === 'cmd' ? cp.copied : cp.copy,
      cmdCopyColor: this.state.copied === 'cmd' ? '#C6F94E' : '#C4C8D4',
      goLanding: () => this.setState({ view: 'landing' }),
      goLogin: () => this.setState({ view: 'login' }),
      goApp: () => this.setState({ view: 'app' }),
      toggleLang: () => this.setState(s => ({ lang: s.lang === 'pt' ? 'en' : 'pt' })),
      toggleTheme: () => this.setState(s => ({ theme: s.theme === 'dark' ? 'light' : 'dark' })),
      genInput: this.state.genInput,
      onGenInput: (e) => this.setState({ genInput: e.target.value }),
      onShorten: () => this.shorten(),
      genBusy: this.state.genBusy,
      genDone: this.state.genDone,
      genShort,
      genTarget: target && target.length > 46 ? target.slice(0, 46) + '…' : target,
      onCopyGen: () => this.copy('https://' + genShort, 'gen'),
      genCopyLabel: this.state.copied === 'gen' ? cp.copied : cp.copy,
      genCopyColor: this.state.copied === 'gen' ? '#C6F94E' : '#E8EAF2',
      ...this.panelVals(t, cp),
    };
  }

  panelVals(t, cp) {
    const pal = this.palette();
    const P = t.panel;
    const tab = this.state.tab;
    const isPt = this.state.lang === 'pt';
    const d = this.data();
    const navSty = (id) => tab === id ? { c: pal.brand, bg: pal.wash } : { c: pal.muted, bg: 'transparent' };
    const setTab = (id) => this.setState({ tab: id, createOpen: false });

    const mkLink = (l) => ({
      ...l,
      short: 'q.rk/' + l.code,
      clicksF: this.fmt(l.clicks),
      tagObjs: l.tags.map(nm => ({ name: nm, color: d.tagColors[nm] || pal.muted, bg: (d.tagColors[nm] || pal.muted) + '22' })),
      copyLabel: this.state.copied === l.code ? P.links.copied : P.links.copy,
      copyColor: this.state.copied === l.code ? '#33C971' : pal.muted,
      onCopy: () => this.copy('https://q.rk/' + l.code, l.code),
      onStats: () => this.setState({ tab: 'analytics', statLink: l.code }),
    });
    const linksView = d.links.map(mkLink);
    const folders = ['Marketing', 'Conteúdo'];
    const folderChips = [{ name: P.links.allFolders, count: d.links.length, active: true }]
      .concat(folders.map(f => ({ name: f, count: d.links.filter(l => l.folder === f).length, active: false })))
      .map(c => ({ ...c, chipBg: c.active ? pal.wash : 'transparent', chipBorder: c.active ? 'rgba(198,249,78,.4)' : pal.border, chipColor: c.active ? pal.brand : pal.muted }));
    const ttlLabels = isPt ? ['Nunca', '1 hora', '24 horas', '7 dias', '30 dias'] : ['Never', '1 hour', '24 hours', '7 days', '30 days'];
    const ttlChips = ttlLabels.map((label, i) => ({ label, bg: i === 4 ? pal.wash : 'transparent', border: i === 4 ? 'rgba(198,249,78,.4)' : pal.border, color: i === 4 ? pal.brand : pal.text }));
    const totalClicks = d.links.reduce((a, l) => a + l.clicks, 0);

    const sel = d.links.find(l => l.code === this.state.statLink) || d.links[0];
    const maxDay = Math.max.apply(null, d.perDay);
    const anBars = d.perDay.map(v => ({ h: Math.round(v / maxDay * 100) + '%' }));
    const bar = (arr, color) => arr.map(x => ({ name: x.name, pct: x.pct, w: x.pct + '%', c: color }));
    const anCountry = bar([{ name: 'Brasil', pct: 54 }, { name: isPt ? 'Estados Unidos' : 'United States', pct: 18 }, { name: 'Portugal', pct: 11 }, { name: isPt ? 'Alemanha' : 'Germany', pct: 9 }, { name: isPt ? 'Outros' : 'Others', pct: 8 }], '#C6F94E');
    const anDevice = bar([{ name: 'Mobile', pct: 61 }, { name: 'Desktop', pct: 33 }, { name: 'Tablet', pct: 6 }], '#4ADEDE');
    const anBrowser = bar([{ name: 'Chrome', pct: 58 }, { name: 'Safari', pct: 24 }, { name: 'Edge', pct: 10 }, { name: 'Firefox', pct: 8 }], '#8B7CF6');
    const anRef = bar([{ name: 'Instagram', pct: 41 }, { name: isPt ? 'Direto' : 'Direct', pct: 27 }, { name: 'Google', pct: 19 }, { name: 'X / Twitter', pct: 13 }], pal.muted);
    const anRecent = [
      { when: isPt ? 'há 2 min' : '2 min ago', geo: 'São Paulo, BR', dev: 'iPhone · Safari', ref: 'Instagram' },
      { when: isPt ? 'há 8 min' : '8 min ago', geo: 'Lisboa, PT', dev: 'MacBook · Chrome', ref: 'Google' },
      { when: isPt ? 'há 14 min' : '14 min ago', geo: 'Berlin, DE', dev: 'Pixel · Chrome', ref: isPt ? 'Direto' : 'Direct' },
      { when: isPt ? 'há 21 min' : '21 min ago', geo: 'New York, US', dev: 'Windows · Edge', ref: 'X / Twitter' },
      { when: isPt ? 'há 33 min' : '33 min ago', geo: 'Rio de Janeiro, BR', dev: 'iPhone · Instagram', ref: 'Instagram' },
    ];
    const an = { total: this.fmt(48213), unique: this.fmt(39120), bots: this.fmt(842), topGeo: 'Brasil', topGeoPct: '54%', topDev: 'Mobile', topDevPct: '61%' };

    const statusDom = (s) => s === 'active' ? { c: '#33C971', label: P.dom.active } : { c: '#FEBC2E', label: P.dom.pending };
    const domainsView = d.domains.map(dm => ({ ...dm, sc: statusDom(dm.status).c, slabel: statusDom(dm.status).label }));
    const roleLabel = (r) => ({ owner: P.mem.owner, admin: P.mem.admin, member: P.mem.member, pending: P.mem.pending })[r];
    const membersView = d.members.map(m => ({ ...m, roleLabel: roleLabel(m.role), isPending: m.role === 'pending', displayName: m.email ? m.name : m.name, subline: m.email || (isPt ? 'Convite pendente' : 'Pending invite') }));
    const tokensView = d.tokens.map(tk => ({ ...tk, scopeStr: tk.scopes.join(' · '), lastUsedF: tk.lastUsed === 'never' ? P.tok.never : (isPt ? 'há ' : '') + tk.lastUsed }));
    const whStatus = (s) => s === 'active' ? { c: '#33C971', label: P.wh.active } : { c: pal.muted, label: P.wh.paused };
    const webhooksView = d.webhooks.map(w => ({ ...w, evStr: w.evs.join(', '), sc: whStatus(w.status).c, slabel: whStatus(w.status).label, urlShort: w.url.length > 46 ? w.url.slice(0, 46) + '…' : w.url }));

    return {
      pal, P,
      isTabLinks: tab === 'links', isTabAnalytics: tab === 'analytics', isTabDomains: tab === 'domains', isTabMembers: tab === 'members', isTabTokens: tab === 'tokens', isTabWebhooks: tab === 'webhooks',
      ns: { links: navSty('links'), analytics: navSty('analytics'), domains: navSty('domains'), members: navSty('members'), tokens: navSty('tokens'), webhooks: navSty('webhooks') },
      navLinks: () => setTab('links'), navAnalytics: () => setTab('analytics'), navDomains: () => setTab('domains'), navMembers: () => setTab('members'), navTokens: () => setTab('tokens'), navWebhooks: () => setTab('webhooks'),
      linksView, folderChips, totalClicksF: this.fmt(totalClicks), linkCount: d.links.length,
      openCreate: () => this.setState({ createOpen: true }), closeCreate: () => this.setState({ createOpen: false }), createOpen: this.state.createOpen,
      statShort: 'q.rk/' + sel.code, statDest: sel.dest, an, anBars, anCountry, anDevice, anBrowser, anRef, anRecent,
      domainsView, membersView, tokensView, webhooksView, seatsUsed: '3 / 5',
      ttlChips, stop: (e) => { if (e && e.stopPropagation) e.stopPropagation(); },
    };
  }
}
</script>