// Service worker mínimo do painel.
//
// Ele existe apenas para o navegador permitir instalar o Sentinela como
// aplicativo. Não guarda cache: o HTML carrega um token novo a cada execução do
// servidor, então toda navegação precisa ir para a rede.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));
self.addEventListener("fetch", () => {
  // Sem respondWith: o navegador segue o fluxo normal, direto para a rede.
});
