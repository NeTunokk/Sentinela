/* Sentinela — painel local integrado
   Execuções  →  Relatório (por execução)
   Execuções  ↔  Ambientes  ↔  Limpeza (ambiente compartilhado)

   Todos os dados vêm da API local servida em 127.0.0.1. O token injetado pelo
   servidor no HTML é enviado em X-Sentinela-Token em cada requisição. */

const TOKEN = window.__SENTINELA_TOKEN__ || '';

const DEFAULT_CONFIG = {
  name: '',
  backup: { provider: 'logical', directory: '', glob: '*.dump' },
  postgres: { version: '16', image: 'postgres', user: 'postgres', database: 'sentinela' },
  docker: {
    memory_mb: 512,
    cpus: 1.0,
    readiness_timeout_secs: 60,
    restore_timeout_secs: 600,
    data_tmpfs_mb: 1024
  },
  tests: { directory: 'tests', statement_timeout_ms: 5000 }
};

const state = {
  envs: [],
  env: null,
  runs: [],
  runDetail: null,
  backups: [],
  backupsError: null,
  backupsLoading: false,
  resources: [],
  resourcesError: null,
  pending: [],
  newEnv: false
};

const STATUS = {
  ok: { label: 'Aprovada', cls: 'ok' },
  tests_failed: { label: 'Testes falharam', cls: 'fl' },
  restore_failed: { label: 'Restore falhou', cls: 'er' },
  infra_error: { label: 'Erro de infraestrutura', cls: 'er' },
  running: { label: 'Em execução', cls: 'rn' }
};

const TEST_STATUS = {
  passed: { label: 'Aprovada', cls: 'ok' },
  failed: { label: 'Falhou', cls: 'fl' },
  error: { label: 'Erro', cls: 'er' }
};

let routeSequence = 0;
let pollTimer = null;

/* ── Utilidades ─────────────────────────────────────────────── */

function esc(value) {
  return String(value === null || value === undefined ? '' : value).replace(/[&<>"']/g, ch => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
  }[ch]));
}

function badge(status) {
  const meta = STATUS[status] || { label: status || '—', cls: 'mute' };
  return `<span class="bd ${meta.cls}">${esc(meta.label)}</span>`;
}

function testBadge(status) {
  const meta = TEST_STATUS[status] || { label: status || '—', cls: 'mute' };
  return `<span class="bd ${meta.cls}">${esc(meta.label)}</span>`;
}

function shortId(id) {
  return id ? String(id).slice(0, 8) : '—';
}

function formatDuration(ms) {
  if (ms === null || ms === undefined) return '—';
  const seconds = Number(ms) / 1000;
  if (!Number.isFinite(seconds)) return '—';
  return seconds >= 10 ? `${seconds.toFixed(0)} s` : `${seconds.toFixed(1)} s`;
}

function formatMs(ms) {
  return ms === null || ms === undefined ? '—' : `${ms} ms`;
}

function formatBytes(bytes) {
  if (bytes === null || bytes === undefined) return '—';
  let value = Number(bytes);
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(2)} ${units[unit]}`;
}

function formatDate(value) {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString('pt-BR');
}

function runTestSummary(run) {
  if (run.status === 'running' || !run.tests_total) return '—';
  return `${run.tests_passed}/${run.tests_total}`;
}

/* ── API local ──────────────────────────────────────────────── */

async function request(path, options = {}) {
  const headers = { 'X-Sentinela-Token': TOKEN, ...(options.headers || {}) };
  if (options.body && !headers['Content-Type']) headers['Content-Type'] = 'application/json';

  let response;
  try {
    response = await fetch(path, { ...options, headers });
  } catch (error) {
    throw new Error(`Falha de conexão com o servidor local: ${error.message}`);
  }

  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`;
    try {
      const body = await response.json();
      if (body && body.error) message = body.error;
    } catch (_) {
      /* resposta sem JSON */
    }
    throw new Error(message);
  }

  return response;
}

async function api(path, options) {
  const response = await request(path, options);
  const type = response.headers.get('content-type') || '';
  return type.includes('application/json') ? response.json() : response.text();
}

async function apiText(path) {
  return (await request(path)).text();
}

/* ── Carregamento ───────────────────────────────────────────── */

async function loadEnvironments() {
  const data = await api('/api/environments');
  state.envs = data.environments || [];
  if (!state.envs.some(item => item.name === state.env)) {
    state.env = state.envs.length ? state.envs[0].name : null;
  }
}

async function loadRuns() {
  if (!state.env) {
    state.runs = [];
    return;
  }
  const data = await api(`/api/runs?env=${encodeURIComponent(state.env)}`);
  state.runs = data.runs || [];
}

async function loadBackups() {
  state.backups = [];
  state.backupsError = null;
  if (!state.env) return;

  state.backupsLoading = true;
  paint();
  try {
    const data = await api(`/api/environments/${encodeURIComponent(state.env)}/backups`);
    state.backups = data.backups || [];
  } catch (error) {
    state.backupsError = error.message;
  } finally {
    state.backupsLoading = false;
  }
}

async function loadResources() {
  state.resourcesError = null;
  try {
    const data = await api('/api/resources');
    state.resources = data.resources || [];
  } catch (error) {
    state.resources = [];
    state.resourcesError = error.message;
  }
}

async function loadRunDetail(id) {
  try {
    const data = await api(`/api/runs/${encodeURIComponent(id)}`);
    state.runDetail = data;
    if (data.pending && !pollTimer) pollRun(id);
  } catch (error) {
    state.runDetail = { notFound: true, error: error.message };
  }
}

/* ── Rotas ──────────────────────────────────────────────────── */

function currentRoute() {
  const raw = window.location.hash.replace(/^#\/?/, '');
  const [name, id] = raw.split('/');
  if (name === 'relatorio' && id) return { name: 'relatorio', id };
  if (name === 'ambientes') return { name: 'ambientes' };
  if (name === 'limpeza') return { name: 'limpeza' };
  return { name: 'execucoes' };
}

function setActiveNav(route) {
  const active = route.name === 'relatorio' ? 'execucoes' : route.name;
  document.querySelectorAll('.nv').forEach(link => {
    const on = link.dataset.nav === active;
    link.classList.toggle('on', on);
    if (on) link.setAttribute('aria-current', 'page');
    else link.removeAttribute('aria-current');
  });
}

/* ── Telas ──────────────────────────────────────────────────── */

function envOptions() {
  return state.envs
    .map(env => `<option value="${esc(env.name)}"${env.name === state.env ? ' selected' : ''}>${esc(env.name)}</option>`)
    .join('');
}

function execucoesView() {
  const env = state.env;

  if (!env) {
    return `
      <header class="page-head end">
        <h1>Execuções</h1>
      </header>
      <section class="card">
        <h2>Nenhum ambiente configurado</h2>
        <p class="note">Crie um ambiente apontando para o diretório de backups PostgreSQL para começar.</p>
        <p style="margin-top:14px"><a class="l" href="#/ambientes">Ir para Ambientes →</a></p>
      </section>`;
  }

  const pending = state.pending.filter(run => run.environment === env);
  const runs = [...pending, ...state.runs.filter(run => run.environment === env)];
  const last = runs[0];
  const testsSummary = last ? runTestSummary(last) : '—';

  const rows = runs.length
    ? runs.map(run => {
      const pendingRun = run.status === 'running';
      const report = pendingRun
        ? '<span class="pending">Gerando…</span>'
        : `<a class="l" href="#/relatorio/${esc(run.id)}">Ver relatório</a>`;
      return `<tr>
        <td><span class="m" title="${esc(run.id)}">${esc(shortId(run.id))}</span></td>
        <td>${run.backup_id ? esc(run.backup_id) : '—'}</td>
        <td>${badge(run.status)}</td>
        <td>${runTestSummary(run)}</td>
        <td>${pendingRun ? '—' : formatDuration(run.total_duration_ms)}</td>
        <td>${report}</td>
      </tr>`;
    }).join('')
    : '<tr><td colspan="6"><span class="empty">Nenhuma execução neste ambiente ainda.</span></td></tr>';

  return `
    <header class="page-head end">
      <h1>Execuções</h1>
      <div class="head-actions">
        <label class="lb">Ambiente
          <select id="env-select" class="in" style="min-width:180px">${envOptions()}</select>
        </label>
        <button type="button" class="btn" data-action="start-run" ${state.pending.length ? 'disabled' : ''}>Executar prova de restauração</button>
      </div>
    </header>

    <div class="stats">
      <div class="card stat">
        <div class="lb">Última execução</div>
        <div class="stat-value">${last ? badge(last.status) : '<span class="bd mute">—</span>'}</div>
      </div>
      <div class="card stat">
        <div class="lb">Backup testado</div>
        <div class="stat-value"><span class="m">${last && last.backup_id ? esc(last.backup_id) : '—'}</span></div>
      </div>
      <div class="card stat">
        <div class="lb">Testes SQL</div>
        <div class="stat-value">${testsSummary}</div>
      </div>
      <div class="card stat">
        <div class="lb">Duração total</div>
        <div class="stat-value">${last && last.status !== 'running' ? formatDuration(last.total_duration_ms) : '—'}</div>
      </div>
    </div>

    <section class="card">
      <h2>Histórico</h2>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th scope="col">ID</th>
              <th scope="col">Backup</th>
              <th scope="col">Status</th>
              <th scope="col">Testes</th>
              <th scope="col">Duração</th>
              <th scope="col">Relatório</th>
            </tr>
          </thead>
          <tbody>${rows}</tbody>
        </table>
      </div>
    </section>
  `;
}

function relatorioView(detail, id) {
  const back = '<a class="l" href="#/execucoes">← Execuções</a>';

  if (!detail) {
    return `${back}
      <section class="card">
        <h2>Carregando relatório</h2>
        <p class="note">Consultando a execução <span class="m">${esc(shortId(id))}</span>…</p>
      </section>`;
  }

  if (detail.notFound) {
    return `${back}
      <section class="card">
        <h2>Relatório não encontrado</h2>
        <p class="note">${esc(detail.error || `A execução ${shortId(id)} não existe neste histórico.`)}</p>
      </section>`;
  }

  const run = detail.run;

  if (detail.pending || run.status === 'running') {
    return `${back}
      <header class="page-head">
        <div class="head-title">
          <h1>Relatório ${esc(shortId(run.id))}</h1>
          ${badge('running')}
          <span class="envtag">${esc(run.environment)}</span>
        </div>
      </header>
      <section class="card">
        <h2>Relatório em preparação</h2>
        <p class="note">A execução está em andamento. Os dados serão atualizados automaticamente.</p>
        <p class="note">Backup: <span class="m">${run.backup_id ? esc(run.backup_id) : 'selecionado pelo Sentinela'}</span></p>
      </section>`;
  }

  const report = detail.report;
  const restoreFailed = run.status === 'restore_failed';
  const bad = restoreFailed || !report || !report.verified;

  const timings = detail.timings;
  const timingRows = timings
    ? [
      ['Descoberta e SHA-256', timings.discovery_ms],
      ['Subida e readiness', timings.startup_ms],
      ['Validação e restore', timings.restore_ms],
      ['Testes SQL', timings.tests_ms]
    ].map(([label, value]) => `<tr><td>${label}</td><td>${formatMs(value)}</td></tr>`).join('')
    : '<tr><td colspan="2"><span class="empty">Tempos detalhados não registrados.</span></td></tr>';

  const totalRow = timings
    ? `<tr class="total"><td><b>Total</b></td><td><b>${formatDuration(timings.total_ms)}</b></td></tr>`
    : (run.total_duration_ms !== null && run.total_duration_ms !== undefined
      ? `<tr class="total"><td><b>Total</b></td><td><b>${formatDuration(run.total_duration_ms)}</b></td></tr>`
      : '');

  const tests = detail.tests || [];
  const testRows = tests.length
    ? tests.map(test => `<tr>
        <td><span class="m">${esc(test.name)}</span></td>
        <td>${testBadge(test.status)}</td>
        <td>${test.value === null || test.value === undefined ? '—' : esc(test.value)}</td>
        <td>${test.duration_ms} ms</td>
      </tr>`).join('')
    : `<tr><td colspan="4"><span class="empty">Testes SQL não executados${restoreFailed ? ' — a restauração falhou antes dos testes.' : '.'}</span></td></tr>`;

  const verificationNote = bad
    ? (report && report.verification_error
      ? `<p class="note">${esc(report.verification_error)}</p>`
      : (restoreFailed
        ? '<p class="note">O dump não passou na validação de integridade e a restauração foi interrompida.</p>'
        : '<p class="note">O relatório assinado não está disponível para esta execução.</p>'))
    : '';

  const title = restoreFailed
    ? 'Restauração falhou'
    : (!report ? 'Relatório indisponível' : (report.verified ? 'Integridade verificada' : 'Falha na verificação'));

  const errorNote = run.error_message
    ? `<section class="card warn"><h2>Erro registrado</h2><p class="note">${esc(run.error_message)}</p></section>`
    : '';

  const canOpen = Boolean(report && report.verified);

  return `${back}
    <header class="page-head">
      <div class="head-title">
        <h1>Relatório ${esc(shortId(run.id))}</h1>
        ${badge(run.status)}
        <span class="envtag">${esc(run.environment)}</span>
      </div>
      <div class="head-actions">
        <button type="button" class="btn b2" data-action="view-json" data-id="${esc(run.id)}" ${canOpen ? '' : 'disabled'}>Ver JSON</button>
        <button type="button" class="btn" data-action="open-html" data-id="${esc(run.id)}" ${canOpen ? '' : 'disabled'}>Abrir HTML</button>
      </div>
    </header>

    <section class="card integrity${bad ? ' bad' : ''}">
      <h2>${title}</h2>
      <div class="integrity-grid">
        <div>
          <div class="lb">SHA-256 do JSON (recalculado)</div>
          <div class="m hash">${report && report.sha256 ? esc(report.sha256) : '—'}</div>
        </div>
        <div>
          <div class="lb">Assinatura Ed25519</div>
          <div class="sig">${report ? (report.verified ? 'Válida' : 'Inválida') : '—'}</div>
        </div>
        <div>
          <div class="lb">Backup</div>
          <div class="m" style="margin-top:6px">${detail.backup && detail.backup.id ? esc(detail.backup.id) : (run.backup_id ? esc(run.backup_id) : '—')}</div>
        </div>
      </div>
      ${verificationNote}
    </section>

    ${errorNote}

    <div class="report-grid">
      <section class="card">
        <h2>Tempos</h2>
        <div class="table-wrap">
          <table>
            <thead>
              <tr><th scope="col">Etapa</th><th scope="col">Duração</th></tr>
            </thead>
            <tbody>
              ${timingRows}
              ${totalRow}
            </tbody>
          </table>
        </div>
      </section>

      <section class="card">
        <h2>Testes SQL</h2>
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th scope="col">Teste</th>
                <th scope="col">Status</th>
                <th scope="col">Valor</th>
                <th scope="col">Duração</th>
              </tr>
            </thead>
            <tbody>${testRows}</tbody>
          </table>
        </div>
      </section>
    </div>
  `;
}

function field(label, name, value, options = {}) {
  const type = options.type || 'text';
  const step = options.step ? ` step="${options.step}"` : '';
  const disabled = options.disabled ? ' disabled' : '';
  return `<label class="lb">${esc(label)}
    <input class="in" type="${type}" name="${esc(name)}" value="${esc(value)}"${step}${disabled}>
  </label>`;
}

function ambientesView() {
  const creating = state.newEnv;
  const selected = state.envs.find(item => item.name === state.env);

  if (!creating && !selected) {
    return `
      <header class="page-head">
        <h1>Ambientes</h1>
      </header>
      <section class="card">
        <h2>Nenhum ambiente configurado</h2>
        <p class="note">Um ambiente descreve onde estão os backups, os testes SQL e qual imagem PostgreSQL usar.</p>
        <p style="margin-top:14px"><button type="button" class="btn" data-action="new-env">Criar ambiente</button></p>
      </section>`;
  }

  const cfg = creating ? DEFAULT_CONFIG : selected.config;

  if (!cfg) {
    return `
      <header class="page-head">
        <div class="head-title">
          <h1>Ambientes</h1>
          <span class="envtag">${esc(selected.name)}</span>
        </div>
      </header>
      <section class="card">
        <h2>Configuração inválida</h2>
        <p class="note">${esc(selected.error || 'Não foi possível carregar o YAML deste ambiente.')}</p>
      </section>`;
  }

  const backupsRows = state.backups.length
    ? state.backups.map(backup => {
      const validation = backup.host_restore_list_ok === true
        ? '<span class="bd ok">Válido</span>'
        : (backup.host_restore_list_ok === false
          ? '<span class="bd fl">Inválido</span>'
          : '<span class="bd mute">n/d</span>');
      return `<tr>
        <td><span class="m" title="${esc(backup.sha256)}">${esc(backup.id)}</span></td>
        <td>${formatBytes(backup.size_bytes)}</td>
        <td>${formatDate(backup.modified_at)}</td>
        <td>${validation}</td>
        <td><button type="button" class="link" data-action="test-backup" data-backup="${esc(backup.id)}">Testar</button></td>
      </tr>`;
    }).join('')
    : `<tr><td colspan="5"><span class="empty">${state.backupsLoading ? 'Calculando SHA-256 dos backups…' : 'Nenhum backup encontrado no diretório configurado.'}</span></td></tr>`;

  const backupsNote = state.backupsError
    ? `<p class="note" style="color:#FCA5A5">${esc(state.backupsError)}</p>`
    : '';

  const envSelector = state.envs.length
    ? `<label class="lb">Ambiente
        <select id="env-select" class="in" style="min-width:180px"${creating ? ' disabled' : ''}>${envOptions()}</select>
      </label>`
    : '';

  const headerActions = creating
    ? '<button type="button" class="btn b2" data-action="cancel-new-env">Cancelar</button>'
    : `<button type="button" class="btn b2" data-action="new-env">Novo ambiente</button>
       <button type="button" class="btn b2" data-action="remove-env">Remover ambiente</button>`;

  return `
    <header class="page-head end">
      <div class="head-title">
        <h1>${creating ? 'Novo ambiente' : 'Ambientes'}</h1>
        <span class="envtag">${creating ? 'novo' : esc(selected.name)}</span>
      </div>
      <div class="head-actions">
        ${envSelector}
        ${headerActions}
      </div>
    </header>

    <div class="env-grid">
      <section class="card">
        <h2>Configuração</h2>
        <form id="env-form" class="config-form" novalidate>
          ${field('Nome', 'name', creating ? '' : cfg.name, { disabled: !creating })}
          <label class="lb">Provider de backup
            <select class="in" name="provider">
              <option value="logical"${cfg.backup.provider === 'logical' ? ' selected' : ''}>logical (pg_dump -Fc)</option>
              <option value="pgbackrest"${cfg.backup.provider === 'pgbackrest' ? ' selected' : ''}>pgbackrest</option>
            </select>
          </label>
          ${field('Diretório de backups', 'backupDirectory', cfg.backup.directory)}
          ${field('Padrão de arquivos (glob)', 'backupGlob', cfg.backup.glob)}
          ${field('Diretório de testes', 'testsDirectory', cfg.tests.directory)}

          <div class="config-grid">
            ${field('Postgres (versão)', 'postgresVersion', cfg.postgres.version)}
            ${field('Imagem', 'postgresImage', cfg.postgres.image)}
            ${field('Usuário', 'postgresUser', cfg.postgres.user)}
            ${field('Banco', 'postgresDatabase', cfg.postgres.database)}
            ${field('Memória (MB)', 'memoryMb', cfg.docker.memory_mb, { type: 'number' })}
            ${field('CPUs', 'cpus', cfg.docker.cpus, { type: 'number', step: '0.1' })}
            ${field('Readiness (s)', 'readinessTimeout', cfg.docker.readiness_timeout_secs, { type: 'number' })}
            ${field('Restore timeout (s)', 'restoreTimeout', cfg.docker.restore_timeout_secs, { type: 'number' })}
            ${field('tmpfs de dados (MB)', 'tmpfsMb', cfg.docker.data_tmpfs_mb, { type: 'number' })}
            ${field('Statement timeout (ms)', 'statementTimeout', cfg.tests.statement_timeout_ms, { type: 'number' })}
          </div>

          <button type="submit" class="btn">${creating ? 'Criar ambiente' : 'Salvar ambiente'}</button>
        </form>
      </section>

      <section class="card">
        <h2>Backups encontrados</h2>
        ${backupsNote}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th scope="col">Nome</th>
                <th scope="col">Tamanho</th>
                <th scope="col">Modificado</th>
                <th scope="col">Validação</th>
                <th scope="col">Ação</th>
              </tr>
            </thead>
            <tbody>${backupsRows}</tbody>
          </table>
        </div>
      </section>
    </div>
  `;
}

function limpezaView() {
  const resources = state.resources;
  const count = kind => resources.filter(item => item.kind === kind).length;
  const typeLabel = kind => ({ container: 'Container', network: 'Rede', volume: 'Volume' }[kind] || kind);

  const rows = resources.length
    ? resources.map(item => {
      const runCell = item.run_id
        ? `<a class="l" href="#/relatorio/${esc(item.run_id)}"><span class="m" title="${esc(item.run_id)}">${esc(shortId(item.run_id))}</span></a>`
        : '<span class="m">—</span>';
      return `<tr>
        <td>${esc(typeLabel(item.kind))}</td>
        <td><span class="m">${esc(item.name)}</span></td>
        <td>${runCell}</td>
        <td>${item.role ? `<span class="bd mute">${esc(item.role)}</span>` : '<span class="bd er">Órfão</span>'}</td>
      </tr>`;
    }).join('')
    : `<tr><td colspan="4"><span class="empty">${state.resourcesError ? esc(state.resourcesError) : 'Nenhum recurso do Sentinela encontrado no Docker local.'}</span></td></tr>`;

  return `
    <header class="page-head">
      <div class="head-title">
        <h1>Limpeza</h1>
      </div>
      <button type="button" class="btn" data-action="remove-resources" ${resources.length ? '' : 'disabled'}>Remover recursos do Sentinela</button>
    </header>

    <div class="stats">
      <div class="card stat">
        <div class="lb">Containers</div>
        <div class="stat-value">${count('container')}</div>
      </div>
      <div class="card stat">
        <div class="lb">Redes</div>
        <div class="stat-value">${count('network')}</div>
      </div>
      <div class="card stat">
        <div class="lb">Volumes</div>
        <div class="stat-value">${count('volume')}</div>
      </div>
    </div>

    <section class="card">
      <h2>Recursos encontrados</h2>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th scope="col">Tipo</th>
              <th scope="col">Nome</th>
              <th scope="col">Execução</th>
              <th scope="col">Papel</th>
            </tr>
          </thead>
          <tbody>${rows}</tbody>
        </table>
      </div>
    </section>

    <section class="card warn">
      <h2>O que será removido</h2>
      <p class="note">Somente containers, redes e volumes com o label sentinela=true. A limpeza é global e não afeta o histórico, os relatórios assinados nem os arquivos de backup.</p>
    </section>
  `;
}

/* ── Renderização ───────────────────────────────────────────── */

function paint() {
  const route = currentRoute();
  const view = document.getElementById('view');
  setActiveNav(route);

  if (route.name === 'relatorio') view.innerHTML = relatorioView(state.runDetail, route.id);
  else if (route.name === 'ambientes') view.innerHTML = ambientesView();
  else if (route.name === 'limpeza') view.innerHTML = limpezaView();
  else view.innerHTML = execucoesView();

  const titles = { execucoes: 'Execuções', ambientes: 'Ambientes', limpeza: 'Limpeza' };
  document.title = route.name === 'relatorio'
    ? `Relatório ${shortId(route.id)} · Sentinela`
    : `${titles[route.name]} · Sentinela`;
}

async function refreshRoute() {
  const sequence = ++routeSequence;
  const route = currentRoute();
  setActiveNav(route);

  if (route.name === 'relatorio' && state.runDetail && state.runDetail.run && state.runDetail.run.id !== route.id) {
    state.runDetail = null;
  }

  try {
    if (route.name === 'relatorio') {
      await loadRunDetail(route.id);
    } else if (route.name === 'ambientes') {
      await loadEnvironments();
      if (state.newEnv) {
        state.backups = [];
        state.backupsError = null;
      } else {
        await loadBackups();
      }
    } else if (route.name === 'limpeza') {
      await loadResources();
    } else {
      await loadEnvironments();
      await loadRuns();
    }

    if (sequence !== routeSequence) return;
    paint();
  } catch (error) {
    if (sequence !== routeSequence) return;
    document.getElementById('view').innerHTML = `
      <section class="card">
        <h2>Não foi possível carregar a tela</h2>
        <p class="note">${esc(error.message)}</p>
        <p class="note">Confirme se o servidor local do Sentinela continua em execução.</p>
      </section>`;
  }
}

/* ── Ações ──────────────────────────────────────────────────── */

async function startRun(requestedBackup) {
  if (!state.env) {
    toast('Selecione um ambiente antes de executar.', 'err');
    return;
  }
  if (state.pending.length) {
    toast('Já existe uma execução em andamento.', 'err');
    return;
  }

  const environment = state.env;
  try {
    const data = await api('/api/runs', {
      method: 'POST',
      body: JSON.stringify({ environment, backup: requestedBackup || null })
    });

    state.pending.push({
      id: data.run_id,
      environment,
      backup_id: requestedBackup || null,
      status: 'running',
      started_at: new Date().toISOString(),
      total_duration_ms: null,
      tests_total: 0,
      tests_passed: 0
    });

    toast(`Prova de restauração iniciada (${shortId(data.run_id)}).`);

    if (currentRoute().name !== 'execucoes') window.location.hash = '#/execucoes';
    else paint();

    pollRun(data.run_id);
  } catch (error) {
    toast(`Não foi possível iniciar: ${error.message}`, 'err');
  }
}

function pollRun(id) {
  stopPolling();

  const tick = async () => {
    try {
      const detail = await api(`/api/runs/${encodeURIComponent(id)}`);
      if (state.runDetail && state.runDetail.run && state.runDetail.run.id === id) {
        state.runDetail = detail;
      }

      if (!detail.pending && detail.run.status !== 'running') {
        state.pending = state.pending.filter(item => item.id !== id);
        stopPolling();
        const status = STATUS[detail.run.status] || { label: detail.run.status };
        toast(
          `Execução ${shortId(id)} concluída: ${status.label}.`,
          detail.run.status === 'ok' ? 'ok' : 'err'
        );
        await refreshRoute();
        return;
      }

      const pending = state.pending.find(item => item.id === id);
      if (pending && detail.run) {
        pending.started_at = detail.run.started_at;
        pending.backup_id = detail.run.backup_id || pending.backup_id;
      }

      if (currentRoute().name === 'execucoes'
        || (currentRoute().name === 'relatorio' && currentRoute().id === id)) {
        paint();
      }
      pollTimer = window.setTimeout(tick, 2000);
    } catch (error) {
      state.pending = state.pending.filter(item => item.id !== id);
      stopPolling();
      toast(`Não foi possível acompanhar a execução: ${error.message}`, 'err');
      await refreshRoute();
    }
  };

  pollTimer = window.setTimeout(tick, 1200);
}

function stopPolling() {
  if (pollTimer) {
    window.clearTimeout(pollTimer);
    pollTimer = null;
  }
}

async function saveEnv(form) {
  const value = name => String(form.elements[name] && form.elements[name].value || '').trim();
  const number = name => Number(value(name));

  const name = value('name');
  if (!name) {
    toast('Informe um nome para o ambiente.', 'err');
    return;
  }
  if (!/^[A-Za-z0-9._-]+$/.test(name) || name.startsWith('.')) {
    toast('Nome inválido: use letras, números, "-", "_" ou "." (sem começar com ponto).', 'err');
    return;
  }
  if (!value('backupDirectory')) {
    toast('Informe o diretório de backups.', 'err');
    return;
  }
  if (!value('testsDirectory')) {
    toast('Informe o diretório de testes.', 'err');
    return;
  }

  const config = {
    name,
    backup: {
      provider: value('provider') || 'logical',
      directory: value('backupDirectory'),
      glob: value('backupGlob') || '*.dump'
    },
    postgres: {
      version: value('postgresVersion'),
      image: value('postgresImage'),
      user: value('postgresUser'),
      database: value('postgresDatabase')
    },
    docker: {
      memory_mb: number('memoryMb'),
      cpus: number('cpus'),
      readiness_timeout_secs: number('readinessTimeout'),
      restore_timeout_secs: number('restoreTimeout'),
      data_tmpfs_mb: number('tmpfsMb')
    },
    tests: {
      directory: value('testsDirectory'),
      statement_timeout_ms: number('statementTimeout')
    }
  };

  try {
    await api(`/api/environments/${encodeURIComponent(name)}`, {
      method: 'PUT',
      body: JSON.stringify(config)
    });
    state.newEnv = false;
    state.env = name;
    await loadEnvironments();
    toast(`Ambiente ${name} salvo.`, 'ok');
    await refreshRoute();
  } catch (error) {
    toast(`Não foi possível salvar: ${error.message}`, 'err');
  }
}

function removeEnv() {
  const env = state.env;
  if (!env) return;

  confirmDialog({
    title: `Remover ambiente ${env}?`,
    body: `A configuração do ambiente ${esc(env)} será removida. O histórico de execuções, os relatórios assinados e os arquivos de backup são preservados.`,
    confirmLabel: 'Remover ambiente',
    onConfirm: async () => {
      try {
        await api(`/api/environments/${encodeURIComponent(env)}`, { method: 'DELETE' });
        await loadEnvironments();
        toast(`Ambiente ${env} removido. O histórico foi preservado.`, 'ok');
        await refreshRoute();
      } catch (error) {
        toast(`Não foi possível remover: ${error.message}`, 'err');
      }
    }
  });
}

function removeResources() {
  if (!state.resources.length) {
    toast('Nenhum recurso para remover.');
    return;
  }

  confirmDialog({
    title: 'Remover recursos do Sentinela?',
    body: `Serão removidos ${state.resources.length} recurso(s) com o label sentinela=true. A limpeza é global; o histórico, os relatórios assinados e os arquivos de backup são preservados.`,
    confirmLabel: `Remover ${state.resources.length} recurso(s)`,
    onConfirm: async () => {
      try {
        const result = await api('/api/cleanup', { method: 'POST' });
        const removed = result.containers_removed + result.networks_removed + result.volumes_removed;
        toast(`${removed} recurso(s) removido(s).`, result.errors && result.errors.length ? 'err' : 'ok');
        if (result.errors && result.errors.length) {
          toast(`${result.errors.length} recurso(s) não puderam ser removidos.`, 'err');
        }
        await loadResources();
        paint();
      } catch (error) {
        toast(`Não foi possível limpar: ${error.message}`, 'err');
      }
    }
  });
}

async function showJson(id) {
  try {
    const json = await apiText(`/api/runs/${encodeURIComponent(id)}/report.json`);
    openModal({
      title: `Relatório ${shortId(id)} — JSON verificado`,
      body: `<pre class="json">${esc(json)}</pre>`,
      actions: [
        { label: 'Copiar JSON', variant: 'b2', onClick: () => copyText(json) },
        { label: 'Fechar' }
      ]
    });
  } catch (error) {
    toast(`Não foi possível abrir o JSON: ${error.message}`, 'err');
  }
}

async function openHtmlReport(id) {
  const win = window.open('', '_blank');
  if (!win) {
    toast('O navegador bloqueou a nova janela.', 'err');
    return;
  }

  try {
    const html = await apiText(`/api/runs/${encodeURIComponent(id)}/report.html`);
    win.document.open();
    win.document.write(html);
    win.document.close();
  } catch (error) {
    win.close();
    toast(`Não foi possível abrir o HTML: ${error.message}`, 'err');
  }
}

function copyText(text) {
  const fallback = () => {
    const area = document.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand('copy');
    area.remove();
    toast(ok ? 'JSON copiado para a área de transferência.' : 'Não foi possível copiar automaticamente.', ok ? 'ok' : 'err');
  };

  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(text)
      .then(() => toast('JSON copiado para a área de transferência.', 'ok'))
      .catch(fallback);
  } else {
    fallback();
  }
}

/* ── Toasts e modal ─────────────────────────────────────────── */

function toast(message, tone) {
  const root = document.getElementById('toasts');
  const element = document.createElement('div');
  element.className = `toast${tone ? ` ${tone}` : ''}`;
  element.textContent = message;
  root.appendChild(element);

  window.setTimeout(() => {
    element.classList.add('out');
    window.setTimeout(() => element.remove(), 220);
  }, 3800);
}

let escapeHandler = null;

function openModal({ title, body, actions = [] }) {
  const root = document.getElementById('modal-root');
  root.innerHTML = `
    <div class="modal-backdrop" data-close="1">
      <div class="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title">
        <h2 id="modal-title">${esc(title)}</h2>
        <div class="modal-body">${body}</div>
        <div class="modal-actions"></div>
      </div>
    </div>`;

  const actionsEl = root.querySelector('.modal-actions');

  actions.forEach(action => {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = `btn${action.variant ? ` ${action.variant}` : ''}`;
    button.textContent = action.label;
    button.addEventListener('click', () => {
      const result = action.onClick ? action.onClick() : undefined;
      if (result !== false) closeModal();
    });
    actionsEl.appendChild(button);
  });

  root.querySelector('.modal-backdrop').addEventListener('mousedown', event => {
    if (event.target.dataset.close) closeModal();
  });

  escapeHandler = event => {
    if (event.key === 'Escape') closeModal();
  };
  document.addEventListener('keydown', escapeHandler);

  if (actionsEl.firstElementChild) actionsEl.firstElementChild.focus();
}

function closeModal() {
  document.getElementById('modal-root').innerHTML = '';
  if (escapeHandler) {
    document.removeEventListener('keydown', escapeHandler);
    escapeHandler = null;
  }
}

function confirmDialog({ title, body, confirmLabel = 'Confirmar', danger = true, onConfirm }) {
  openModal({
    title,
    body: `<p>${body}</p>`,
    actions: [
      { label: 'Cancelar', variant: 'b2' },
      { label: confirmLabel, variant: danger ? 'danger' : 'btn', onClick: onConfirm }
    ]
  });
}

/* ── Eventos ────────────────────────────────────────────────── */

const view = document.getElementById('view');

view.addEventListener('click', event => {
  const trigger = event.target.closest('[data-action]');
  if (!trigger || !view.contains(trigger)) return;

  switch (trigger.dataset.action) {
    case 'start-run':
      startRun();
      break;
    case 'test-backup':
      startRun(trigger.dataset.backup);
      break;
    case 'new-env':
      state.newEnv = true;
      paint();
      break;
    case 'cancel-new-env':
      state.newEnv = false;
      paint();
      break;
    case 'remove-env':
      removeEnv();
      break;
    case 'remove-resources':
      removeResources();
      break;
    case 'view-json':
      showJson(trigger.dataset.id);
      break;
    case 'open-html':
      openHtmlReport(trigger.dataset.id);
      break;
    default:
      break;
  }
});

view.addEventListener('change', event => {
  if (event.target.id === 'env-select') {
    state.env = event.target.value;
    state.newEnv = false;
    refreshRoute();
  }
});

view.addEventListener('submit', event => {
  if (event.target.id === 'env-form') {
    event.preventDefault();
    saveEnv(event.target);
  }
});

window.addEventListener('hashchange', refreshRoute);

document.getElementById('view').innerHTML = '<section class="card"><p class="note">Carregando painel…</p></section>';
refreshRoute();
