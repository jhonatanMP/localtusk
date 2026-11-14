/* LocalTusk — site.
   - Sem rastreamento e sem requisições a terceiros, exceto o botão "Baixar" (página inicial e
     página de download), que consulta a API pública do GitHub para achar o arquivo da versão mais recente.
   - Movimento só com transform/opacity. Com "reduzir movimento" ligado, nada anima. */
(() => {
  "use strict";

  const doc = document.documentElement;
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
  const REPO = "jhonatanMP/localtusk";

  /* ------------------------------------------------------------ Cabeçalho */
  function initHeader() {
    const header = $(".site-header");
    if (!header) return;
    const onScroll = () => header.classList.toggle("scrolled", window.scrollY > 12);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });

    const btn = $(".menu-btn");
    const links = $("#nav-links");
    if (!btn || !links) return;
    const set = (open) => {
      links.classList.toggle("open", open);
      btn.setAttribute("aria-expanded", String(open));
      btn.setAttribute("aria-label", open ? "Fechar menu" : "Abrir menu");
    };
    btn.addEventListener("click", () => set(!links.classList.contains("open")));
    links.addEventListener("click", (e) => { if (e.target.closest("a")) set(false); });
    document.addEventListener("keydown", (e) => { if (e.key === "Escape") set(false); });
    document.addEventListener("click", (e) => { if (!header.contains(e.target)) set(false); });
  }

  /* ------------------------------------------------------------ Abas (galeria) */
  function initTabs() {
    $$("[data-tabs]").forEach((root) => {
      const tabs = $$('[role="tab"]', root);
      const panels = tabs.map((t) => document.getElementById(t.getAttribute("aria-controls")));
      const select = (i, focus) => {
        tabs.forEach((t, n) => {
          const on = n === i;
          t.setAttribute("aria-selected", String(on));
          t.tabIndex = on ? 0 : -1;
          panels[n].hidden = !on;
        });
        if (focus) tabs[i].focus();
        const shot = $(".shot img", panels[i]);
        if (window.gsap && !doc.classList.contains("no-motion") && shot) {
          gsap.fromTo(shot, { opacity: 0, y: 14 }, { opacity: 1, y: 0, duration: 0.45, ease: "power3.out", clearProps: "transform" });
        }
      };
      tabs.forEach((t, i) => {
        t.addEventListener("click", () => select(i));
        t.addEventListener("keydown", (e) => {
          const k = { ArrowRight: 1, ArrowLeft: -1 }[e.key];
          if (k) { e.preventDefault(); select((i + k + tabs.length) % tabs.length, true); }
          if (e.key === "Home") { e.preventDefault(); select(0, true); }
          if (e.key === "End") { e.preventDefault(); select(tabs.length - 1, true); }
        });
      });
    });
  }

  /* ------------------------------------------------------------ Copiar comandos */
  function initCopy() {
    $$("[data-copy]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const target = document.getElementById(btn.dataset.copy);
        if (!target) return;
        const text = target.innerText.replace(/^\s*#.*$/gm, "").replace(/\n{2,}/g, "\n").trim();
        const label = $("span", btn);
        try {
          await navigator.clipboard.writeText(text);
          label.textContent = "Copiado";
        } catch {
          label.textContent = "Selecione e copie";
        }
        setTimeout(() => (label.textContent = "Copiar"), 1800);
      });
    });
  }

  /* ------------------------------------------------------------ Download */
  const RELEASES_URL = `https://github.com/${REPO}/releases`;
  const OS_NAME = { macos: "macOS", windows: "Windows", linux: "Linux" };
  const ARCH_NAME = { macos: { arm64: "Apple Silicon", x64: "Intel" }, linux: { arm64: "ARM64", x64: "x86_64" }, windows: { arm64: "ARM64", x64: "x64" } };
  const fmtSize = (b) => (b > 1e6 ? (b / 1e6).toFixed(1).replace(".", ",") + " MB" : Math.max(1, Math.round(b / 1e3)) + " KB");

  /** Sistema do visitante: "macos", "linux", "windows" ou "" (celular, tablet ou desconhecido). */
  function detectOS() {
    const ua = navigator.userAgent || "";
    if (/android|iphone|ipad|ipod|cros/i.test(ua)) return "";
    const p = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || ua;
    // iPadOS se apresenta como Mac, mas tem tela de toque.
    if (/mac/i.test(p)) return navigator.maxTouchPoints > 1 ? "" : "macos";
    if (/win/i.test(p)) return "windows";
    if (/linux|x11/i.test(p)) return "linux";
    return "";
  }

  /**
   * Arquitetura do visitante: { arch: "arm64" | "x64", sure: boolean }.
   * O Chrome e o Edge informam o chip; o Safari e o Firefox escondem, então no Mac o chute é
   * Apple Silicon (a maioria) e `sure` fica falso para a página oferecer a versão Intel.
   */
  async function detectArch(os) {
    try {
      const uad = navigator.userAgentData;
      if (uad && uad.getHighEntropyValues) {
        const h = await uad.getHighEntropyValues(["architecture"]);
        if (h.architecture) return { arch: /arm/i.test(h.architecture) ? "arm64" : "x64", sure: true };
      }
    } catch { /* segue para os outros sinais */ }
    const hint = `${navigator.userAgent || ""} ${navigator.platform || ""}`;
    if (os === "linux") return { arch: /aarch64|arm64|armv8/i.test(hint) ? "arm64" : "x64", sure: true };
    if (os === "macos") {
      try {
        const gl = document.createElement("canvas").getContext("webgl");
        const ext = gl && gl.getExtension("WEBGL_debug_renderer_info");
        const gpu = ext ? String(gl.getParameter(ext.UNMASKED_RENDERER_WEBGL)) : "";
        if (/apple m\d/i.test(gpu)) return { arch: "arm64", sure: true };
        if (/intel|amd|radeon|nvidia/i.test(gpu)) return { arch: "x64", sure: true };
      } catch { /* sem WebGL */ }
      return { arch: "arm64", sure: false };
    }
    return { arch: "x64", sure: false };
  }

  /** Reconhece o arquivo da release pelo nome: { os, arch, kind } ou null. */
  function classify(name) {
    const n = name.toLowerCase();
    const arch = /aarch64|arm64/.test(n) ? "arm64" : /x86_64|x64|amd64/.test(n) ? "x64" : "";
    if (n.endsWith(".dmg")) return { os: "macos", arch, kind: "dmg" };
    if (n.endsWith(".appimage")) return { os: "linux", arch, kind: "appimage" };
    if (n.endsWith(".deb")) return { os: "linux", arch, kind: "deb" };
    if (n.endsWith(".rpm")) return { os: "linux", arch, kind: "rpm" };
    if (/\.(msi|exe)$/.test(n)) return { os: "windows", arch, kind: "installer" };
    return null;
  }

  /** Última release publicada, com os arquivos já classificados. `null` se ainda não houver nenhuma. */
  async function loadRelease() {
    const KEY = "localtusk-release";
    try {
      const cached = JSON.parse(sessionStorage.getItem(KEY) || "null");
      if (cached && Date.now() - cached.at < 10 * 60 * 1000) return cached.release;
    } catch { /* cache ilegível: consulta de novo */ }
    // Lista com 1 item: devolve [] (200) quando ainda não há versão publicada, em vez de um 404.
    const res = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=1`, { headers: { Accept: "application/vnd.github+json" } });
    if (!res.ok) throw new Error(String(res.status));
    const rel = (await res.json()).find((r) => !r.draft);
    const release = rel
      ? {
          tag: rel.tag_name || rel.name,
          publishedAt: rel.published_at,
          url: rel.html_url,
          assets: (rel.assets || [])
            .filter((a) => !/\.(sig|sha256|txt|json)$/i.test(a.name))
            .map((a) => ({ name: a.name, url: a.browser_download_url, size: a.size, info: classify(a.name) })),
        }
      : null;
    try { sessionStorage.setItem(KEY, JSON.stringify({ at: Date.now(), release })); } catch { /* armazenamento bloqueado */ }
    return release;
  }

  /** O arquivo recomendado para este visitante (dmg no Mac, AppImage no Linux). */
  function pickAsset(release, os, arch) {
    const kind = os === "macos" ? "dmg" : os === "linux" ? "appimage" : os === "windows" ? "installer" : "";
    if (!release || !kind) return null;
    return release.assets.find((a) => a.info && a.info.os === os && a.info.kind === kind && a.info.arch === arch) || null;
  }

  /** Botões "Baixar" da página inicial: texto pelo sistema e, havendo release, link direto. */
  async function initCtas() {
    const ctas = $$("[data-cta]");
    if (!ctas.length) return;
    const os = detectOS();
    const supported = os === "macos" || os === "linux" || os === "windows";
    ctas.forEach((el) => {
      const label = $("[data-cta-label]", el);
      if (label) label.textContent = supported ? `Baixar para ${OS_NAME[os]}` : "Baixar o LocalTusk";
    });
    const meta = $$("[data-cta-meta]");
    if (!supported) return;
    try {
      const [release, { arch }] = await Promise.all([loadRelease(), detectArch(os)]);
      const asset = pickAsset(release, os, arch);
      if (!asset) return; // sem instalador publicado: o botão continua levando à página de download
      ctas.forEach((el) => {
        el.href = asset.url;
        el.rel = "noopener";
      });
      meta.forEach((el) => {
        el.textContent = "";
        el.append(`${release.tag} · ${OS_NAME[os]} ${ARCH_NAME[os][arch]} · ${fmtSize(asset.size)} · `);
        const more = document.createElement("a");
        more.href = el.dataset.moreHref || "baixar/";
        more.textContent = "outros sistemas";
        el.append(more);
      });
    } catch {
      /* sem rede ou limite do GitHub: o botão segue para a página de download */
    }
  }

  /** Página de download: destaca o sistema do visitante e liga cada botão ao arquivo da release. */
  async function initDownload() {
    const page = $("[data-download]");
    if (!page) return;
    const os = detectOS();
    const { arch, sure } = os ? await detectArch(os) : { arch: "", sure: false };

    $$(".os-card").forEach((c) => {
      if (c.dataset.os === os) {
        c.classList.add("rec");
        const tag = $(".you", c);
        if (tag) tag.hidden = false;
      }
    });
    // Dentro do cartão do sistema, o arquivo da arquitetura do visitante vira o botão principal.
    $$(".os-card.rec [data-asset]").forEach((a) => {
      const [, assetArch, kind] = a.dataset.asset.split("-");
      const primary = assetArch === arch && (kind === "dmg" || kind === "appimage" || kind === "installer");
      a.classList.toggle("btn-primary", primary);
      a.classList.toggle("btn-ghost", !primary);
    });
    $$(".os-card.rec .dl-group").forEach((g) => { if (g.dataset.arch === arch) g.parentElement.prepend(g); });
    const note = $("[data-arch-note]");
    if (note) note.hidden = !(os === "macos" && !sure);

    const box = $("#release");
    if (!box) return;
    const title = $("[data-release-title]", box);
    const text = $("[data-release-text]", box);
    const list = $("[data-release-assets]", box);
    try {
      const rel = await loadRelease();
      if (!rel) return; // ainda sem versão publicada: mantém o texto padrão
      const assets = rel.assets;
      title.textContent = `Versão ${rel.tag}`;
      const when = rel.publishedAt ? new Date(rel.publishedAt).toLocaleDateString("pt-BR", { day: "numeric", month: "long", year: "numeric" }) : "";
      text.textContent = assets.length
        ? `Publicada em ${when}. Os botões acima já apontam para esta versão.`
        : `Publicada em ${when}, mas ainda sem instaladores anexados. Veja a página da versão no GitHub.`;

      // Cada botão com data-asset="<sistema>-<arquitetura>-<formato>" vai direto ao arquivo.
      $$("[data-asset]").forEach((btn) => {
        const [bos, barch, bkind] = btn.dataset.asset.split("-");
        const hit = assets.find((a) => a.info && a.info.os === bos && a.info.arch === barch && a.info.kind === bkind);
        if (!hit) {
          btn.classList.add("pending");
          btn.title = "Este arquivo ainda não foi publicado nesta versão";
          return;
        }
        btn.href = hit.url;
        const size = $("[data-size]", btn);
        if (size) size.textContent = fmtSize(hit.size);
      });

      assets.forEach((a) => {
        const link = document.createElement("a");
        link.className = "asset";
        link.href = a.url;
        link.rel = "noopener";
        const name = document.createElement("span");
        name.textContent = a.name;
        const meta = document.createElement("small");
        const i = a.info;
        meta.textContent = `${i ? OS_NAME[i.os] + " · " : ""}${fmtSize(a.size)}`;
        link.append(name, meta);
        list.append(link);
        if (i && i.os === os) link.style.borderColor = "var(--accent)";
      });
      list.hidden = !assets.length;
    } catch {
      text.textContent = "Não foi possível consultar o GitHub agora. Veja as versões diretamente na página de releases.";
    }
  }

  /* ------------------------------------------------------------ Movimento (GSAP) */
  function splitWords(el) {
    const walk = (node) => {
      [...node.childNodes].forEach((child) => {
        if (child.nodeType === 3) {
          const frag = document.createDocumentFragment();
          child.textContent.split(/(\s+)/).forEach((part) => {
            if (!part) return;
            if (/^\s+$/.test(part)) { frag.append(" "); return; }
            const w = document.createElement("span");
            w.className = "word";
            const inner = document.createElement("span");
            inner.className = "word-inner";
            inner.textContent = part;
            w.append(inner);
            frag.append(w);
          });
          child.replaceWith(frag);
        } else if (child.nodeType === 1) walk(child);
      });
    };
    walk(el);
    return $$(".word-inner", el);
  }

  function initMotion() {
    if (!window.gsap || !window.ScrollTrigger) { doc.classList.add("no-motion"); return; }
    gsap.registerPlugin(ScrollTrigger);
    const mm = gsap.matchMedia();

    mm.add("(prefers-reduced-motion: reduce)", () => { doc.classList.add("no-motion"); });

    mm.add("(prefers-reduced-motion: no-preference)", () => {
      doc.classList.remove("no-motion");
      doc.classList.add("motion-ready");

      // Barra de progresso de leitura (só transform).
      gsap.to(".progress", { scaleX: 1, ease: "none", scrollTrigger: { start: 0, end: "max", scrub: 0.2 } });

      // Entrada do hero.
      const h1 = $(".hero h1");
      if (h1) {
        const words = splitWords(h1);
        gsap.set(h1, { opacity: 1 });
        gsap.set(".hero [data-hero]", { opacity: 1 });
        const tl = gsap.timeline({ defaults: { ease: "power3.out" } });
        tl.from(".hero .pill", { y: 14, opacity: 0, duration: 0.6 })
          .from(words, { yPercent: 115, duration: 0.9, stagger: 0.06, ease: "power4.out" }, "-=0.35")
          .from(".hero-sub", { y: 18, opacity: 0, duration: 0.7 }, "-=0.55")
          .from(".hero-cta > *", { y: 14, opacity: 0, duration: 0.55, stagger: 0.08 }, "-=0.45")
          .from(".hero-chips li", { y: 10, opacity: 0, duration: 0.45, stagger: 0.06 }, "-=0.35")
          .from(".stage .window", { y: 70, opacity: 0, rotationX: 14, scale: 0.94, duration: 1.2, ease: "power3.out" }, "-=0.7");
      }

      // Brilhos de fundo: deriva lenta, pausa quando o hero sai da tela.
      const glows = $$(".glow");
      if (glows.length) {
        const drift = gsap.timeline({ repeat: -1, yoyo: true, defaults: { ease: "sine.inOut", duration: 11 } });
        drift.to(".glow-a", { x: 70, y: 30 }, 0).to(".glow-b", { x: -80, y: 50 }, 0);
        ScrollTrigger.create({ trigger: ".hero", start: "top bottom", end: "bottom top", onToggle: (self) => (self.isActive ? drift.play() : drift.pause()) });
      }

      // Janela do app sobe devagar enquanto o hero sai.
      if ($(".stage .window")) {
        gsap.to(".stage .window", { yPercent: -5, ease: "none", scrollTrigger: { trigger: ".stage", start: "top 80%", end: "bottom top", scrub: 0.6 } });
      }

      // Revelar blocos ao rolar.
      if ($("[data-reveal]")) {
        gsap.set("[data-reveal]", { y: 26 });
        ScrollTrigger.batch("[data-reveal]", {
          start: "top 90%",
          once: true,
          onEnter: (batch) => gsap.to(batch, { opacity: 1, y: 0, duration: 0.8, stagger: 0.09, ease: "power3.out", overwrite: true, clearProps: "transform" }),
        });
      }

      // Passos de "como funciona": o número acende ao entrar.
      $$(".steps li").forEach((li) => {
        gsap.from(li, { x: -22, opacity: 0, duration: 0.7, ease: "power3.out", scrollTrigger: { trigger: li, start: "top 88%", once: true } });
      });

      // Fluxo: pacote percorre navegador → Caddy → PHP-FPM → MySQL.
      $$(".lane").forEach((lane, idx) => {
        const row = $(".lane-row", lane);
        const packet = $(".packet", lane);
        const pings = $$(".ping", lane);
        const track = $(".track", lane);
        if (!row || !packet || !track) return;
        const dist = () => track.getBoundingClientRect().width;
        const tl = gsap.timeline({ repeat: -1, repeatDelay: 0.9, delay: idx * 1.1, paused: true });
        tl.fromTo(packet, { x: 0, opacity: 0 }, { x: dist, opacity: 1, duration: 3.2, ease: "none", keyframes: { opacity: [0, 1, 1, 1, 0] } }, 0);
        pings.forEach((p, i) => tl.fromTo(p, { scale: 0.92, opacity: 0.9 }, { scale: 1.12, opacity: 0, duration: 0.7, ease: "power2.out" }, (3.2 / (pings.length - 1)) * i));
        ScrollTrigger.create({ trigger: lane, start: "top 95%", end: "bottom 5%", invalidateOnRefresh: true, onToggle: (self) => (self.isActive ? tl.play() : tl.pause()) });
      });

      // Cartões do bento sobem em cascata (além do reveal).
      $$(".bento .card").forEach((c) => c.addEventListener("pointerenter", () => gsap.to($(".icon-chip", c), { y: -3, duration: 0.25, ease: "power2.out" })));
      $$(".bento .card").forEach((c) => c.addEventListener("pointerleave", () => gsap.to($(".icon-chip", c), { y: 0, duration: 0.3, ease: "power2.out" })));

      return () => doc.classList.remove("motion-ready");
    });
  }

  /* ------------------------------------------------------------ Início */
  initHeader();
  initTabs();
  initCopy();
  initCtas();
  initDownload();
  initMotion();

  // Plano B: se o GSAP não carregar, o conteúdo nunca fica invisível.
  window.addEventListener("load", () => {
    if (!window.gsap) doc.classList.add("no-motion");
  });
})();
