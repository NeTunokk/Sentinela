//! Descoberta do painel instalado como aplicativo (PWA).
//!
//! Quando o Sentinela é instalado como aplicativo pelo Edge/Chrome, o sistema
//! cria um atalho que abre o navegador em modo `--app` na URL do painel. Este
//! módulo procura esse atalho e o executa; se não encontrar, o chamador abre o
//! navegador padrão.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Abre o painel no aplicativo instalado para esta porta.
///
/// Devolve `false` quando não há aplicativo instalado ou quando o atalho não
/// pôde ser executado — nesse caso o chamador deve abrir o navegador padrão.
pub(crate) fn open_installed(port: u16) -> bool {
    let Some(shortcut) = find_installed(port) else {
        return false;
    };
    launch(&shortcut)
}

/// Abre o painel em uma janela de aplicativo do Chrome/Edge (`--app=URL`).
///
/// Não instala nada: usa o navegador já instalado para abrir o painel sem abas
/// e sem barra de endereço. É o comportamento quando não há PWA instalado.
pub(crate) fn open_app_window(url: &str) -> bool {
    let Some(browser) = chromium_browser() else {
        return false;
    };
    let mut command = Command::new(browser);
    command.arg(format!("--app={url}"));

    // No WSL, o navegador é um .exe do Windows: evita o aviso de caminho UNC.
    #[cfg(all(unix, not(target_os = "macos")))]
    if running_in_wsl() {
        command.current_dir("/mnt/c/Windows");
    }

    command.spawn().is_ok()
}

/// Verdadeiro quando o servidor roda dentro do WSL.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn running_in_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/version")
            .map(|version| version.to_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

#[cfg(any(target_os = "windows", target_os = "macos", not(any(windows, unix))))]
pub(crate) fn running_in_wsl() -> bool {
    false
}

/// Compara o texto do atalho com as URLs que o painel usa.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn is_panel_app(text: &str, port: u16) -> bool {
    if !text.contains("--app=") {
        return false;
    }
    text.contains(&format!("--app=http://127.0.0.1:{port}"))
        || text.contains(&format!("--app=http://localhost:{port}"))
}

fn name_matches(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.to_lowercase().contains("sentinela"))
}

/// Navegador baseado em Chromium capaz de abrir uma janela `--app`.
#[cfg(target_os = "windows")]
fn chromium_browser() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for variable in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(variable) {
            let base = PathBuf::from(base);
            candidates.push(base.join(r"Google\Chrome\Application\chrome.exe"));
            candidates.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn chromium_browser() -> Option<PathBuf> {
    if running_in_wsl() {
        for candidate in [
            "/mnt/c/Program Files/Google/Chrome/Application/chrome.exe",
            "/mnt/c/Program Files (x86)/Google/Chrome/Application/chrome.exe",
            "/mnt/c/Program Files/Microsoft/Edge/Application/msedge.exe",
            "/mnt/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
        ] {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    let path = std::env::var_os("PATH")?;
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
    ] {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn chromium_browser() -> Option<PathBuf> {
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

#[cfg(not(any(windows, unix)))]
fn chromium_browser() -> Option<PathBuf> {
    None
}

#[cfg(target_os = "windows")]
fn find_installed(port: u16) -> Option<PathBuf> {
    let mut shortcuts = Vec::new();
    for root in shortcut_roots() {
        collect_shortcuts(&root, 0, &mut shortcuts);
    }

    let mut matches: Vec<(bool, PathBuf)> = shortcuts
        .into_iter()
        .filter(|path| shortcut_is_panel_app(path, port))
        .map(|path| (name_matches(&path), path))
        .collect();

    // Prefere atalhos com "Sentinela" no nome, para não abrir um PWA de outro
    // site que por acaso use a mesma porta.
    matches.sort_by_key(|(named, _)| !*named);
    matches.into_iter().map(|(_, path)| path).next()
}
#[cfg(target_os = "windows")]
fn shortcut_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        roots.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(profile).join("Desktop"));
    }
    if let Ok(public) = std::env::var("PUBLIC") {
        roots.push(PathBuf::from(public).join("Desktop"));
    }
    roots.retain(|root| root.is_dir());
    roots
}

#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
fn collect_shortcuts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_shortcuts(&path, depth + 1, out);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            out.push(path);
        }
    }
}

/// Um atalho `.lnk` guarda o destino e os argumentos como texto UTF-16LE, mas
/// alguns campos podem vir em ANSI. Testa as duas leituras.
#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
fn shortcut_is_panel_app(path: &Path, port: u16) -> bool {
    let Some(bytes) = read_small_file(path) else {
        return false;
    };
    let utf16 = decode_utf16le(&bytes);
    let utf8 = String::from_utf8_lossy(&bytes);

    if is_panel_app(&utf16, port) || is_panel_app(&utf8, port) {
        return true;
    }

    // PWA instalado pelo manifesto: o atalho aponta para `chrome_proxy.exe` /
    // `msedge_proxy.exe` e guarda o app em um bloco binário, sem a URL legível.
    // Aí o nome do atalho ("Sentinela") é a pista que confirma o nosso app.
    let proxy = utf16.contains("chrome_proxy")
        || utf16.contains("msedge_proxy")
        || utf8.contains("chrome_proxy")
        || utf8.contains("msedge_proxy");
    if proxy && name_matches(path) {
        return true;
    }

    // Formato antigo/simples: o atalho carrega `--app-id` como texto.
    let has_app_id = utf16.contains("--app-id=") || utf8.contains("--app-id=");
    has_app_id && name_matches(path)
}

#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
fn read_small_file(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::metadata(path).ok()?;
    // Atalhos são pequenos; ignora qualquer coisa grande demais para ser um.
    if metadata.len() > 1_048_576 {
        return None;
    }
    std::fs::read(path).ok()
}

#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
fn decode_utf16le(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if let Some(character) = char::from_u32(u32::from(unit)) {
            text.push(character);
        }
    }
    text
}

#[cfg(target_os = "windows")]
fn launch(shortcut: &Path) -> bool {
    // `cmd /C <atalho>` executa o .lnk pelo ShellExecute, sem precisar de
    // `start` (e sem o argumento vazio de título, que o WSL descarta).
    Command::new("cmd")
        .arg("/C")
        .arg(shortcut.as_os_str())
        .spawn()
        .is_ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn find_installed(port: u16) -> Option<PathBuf> {
    if running_in_wsl() {
        if let Some(shortcut) = find_windows_shortcut(port) {
            return Some(shortcut);
        }
    }
    find_desktop_entry(port)
}

/// PWA instalado no Windows, visto do WSL através de `/mnt/c`.
#[cfg(all(unix, not(target_os = "macos")))]
fn find_windows_shortcut(port: u16) -> Option<PathBuf> {
    let mut shortcuts = Vec::new();
    for root in windows_shortcut_roots() {
        collect_shortcuts(&root, 0, &mut shortcuts);
    }
    let mut matches: Vec<(bool, PathBuf)> = shortcuts
        .into_iter()
        .filter(|path| shortcut_is_panel_app(path, port))
        .map(|path| (name_matches(&path), path))
        .collect();
    matches.sort_by_key(|(named, _)| !*named);
    matches.into_iter().map(|(_, path)| path).next()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn windows_shortcut_roots() -> Vec<PathBuf> {
    let base = PathBuf::from("/mnt/c/Users");
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if matches!(
            name.as_str(),
            "public" | "default" | "default user" | "all users"
        ) {
            continue;
        }
        let profile = entry.path();
        roots.push(profile.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs"));
        roots.push(profile.join("Desktop"));
    }
    roots.retain(|root| root.is_dir());
    roots
}

#[cfg(all(unix, not(target_os = "macos")))]
fn find_desktop_entry(port: u16) -> Option<PathBuf> {
    let Ok(home) = std::env::var("HOME") else {
        return None;
    };
    let home = PathBuf::from(home);
    let mut entries = Vec::new();
    for root in [home.join(".local/share/applications"), home.join("Desktop")] {
        collect_desktop_files(&root, 0, &mut entries);
    }

    let mut matches: Vec<(bool, PathBuf)> = entries
        .into_iter()
        .filter(|path| desktop_file_is_panel_app(path, port))
        .map(|path| (name_matches(&path), path))
        .collect();
    matches.sort_by_key(|(named, _)| !*named);
    matches.into_iter().map(|(_, path)| path).next()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn collect_desktop_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(&path, depth + 1, out);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("desktop"))
        {
            out.push(path);
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_file_is_panel_app(path: &Path, port: u16) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    if is_panel_app(&contents, port) {
        return true;
    }
    // PWA instalado pelo manifesto: o Exec chama o navegador com `--app-id` e
    // a URL não aparece no arquivo; o nome do app fica na linha `Name=`.
    contents.contains("--app-id=")
        && desktop_name(&contents).is_some_and(|name| name.to_lowercase().contains("sentinela"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_name(contents: &str) -> Option<&str> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix("Name="))
        .map(str::trim)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch(entry: &Path) -> bool {
    if let Some(windows) = windows_shortcut_path(entry) {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", windows.as_str()]);
        if Path::new("/mnt/c/Windows").is_dir() {
            command.current_dir("/mnt/c/Windows");
        }
        return command.spawn().is_ok();
    }
    Command::new("gio").arg("launch").arg(entry).spawn().is_ok()
}

/// Converte `/mnt/c/Users/...` em `C:\Users\...` para abrir no Windows.
#[cfg(all(unix, not(target_os = "macos")))]
fn windows_shortcut_path(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let rest = text.strip_prefix("/mnt/")?;
    let (drive, tail) = rest.split_once('/')?;
    if drive.len() != 1 {
        return None;
    }
    Some(format!(
        "{}:\\{}",
        drive.to_uppercase(),
        tail.replace('/', "\\")
    ))
}

#[cfg(target_os = "macos")]
fn find_installed(_port: u16) -> Option<PathBuf> {
    // No macOS, "Adicionar ao Dock" do Safari cria um app em ~/Applications.
    let home = std::env::var("HOME").ok()?;
    std::fs::read_dir(PathBuf::from(home).join("Applications"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
                && name_matches(path)
        })
}

#[cfg(target_os = "macos")]
fn launch(app: &Path) -> bool {
    Command::new("open").arg(app.as_os_str()).spawn().is_ok()
}

#[cfg(not(any(windows, unix)))]
fn find_installed(_port: u16) -> Option<PathBuf> {
    None
}

#[cfg(not(any(windows, unix)))]
fn launch(_shortcut: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::is_panel_app;

    #[test]
    fn reconhece_atalho_do_painel() {
        let text = r"C:\...\chrome.exe --app=http://127.0.0.1:8787/";
        assert!(is_panel_app(text, 8787));
    }

    #[test]
    fn aceita_localhost() {
        assert!(is_panel_app("--app=http://localhost:9000", 9000));
    }

    #[test]
    fn recusa_outra_porta_e_navegador_normal() {
        assert!(!is_panel_app("--app=http://127.0.0.1:8787/", 8788));
        assert!(!is_panel_app("http://127.0.0.1:8787/", 8787));
        assert!(!is_panel_app("--app=http://example.com/", 8787));
    }
}
