import { useEffect, useState } from "react";
import type { SiteCopy, SiteLocale } from "./types";
import { release } from "./release";

const copy: Record<SiteLocale, SiteCopy> = {
  "zh-CN": {
    nav: { product: "产品", workflow: "工作方式", security: "隐私", faq: "文档", github: "GitHub", download: "下载 macOS 版" },
    hero: {
      kicker: "LOCAL-FIRST MODEL ROUTER FOR CODEX",
      title: "在 ChatGPT 里，直接选择不同厂商的模型。",
      description: "连接 Kimi、DeepSeek、Grok、OpenAI 多账号或兼容网关。配置一次，回到 ChatGPT / Codex 原生模型选择器，用一个按钮切换模型。",
    },
    workflow: [
      { title: "接入供应商", description: "使用官方 API、配置 JSON 或多账号凭据，将你正在使用的模型集中起来。" },
      { title: "启用模型", description: "发现模型后选择要发布的模型，让它们出现在 ChatGPT / Codex 的原生选择器里。" },
      { title: "在 ChatGPT / Codex 选择", description: "点击一个按钮，在不同厂商和模型之间切换，不离开熟悉的工作流。" },
    ],
    faq: [
      { question: "Codex Spur 会修改 ChatGPT.app 吗？", answer: "不会。Spur 通过本地 Responses 兼容代理、模型目录和专用 codex_select provider 接入，不注入或覆盖 ChatGPT.app。" },
      { question: "我的凭据会离开这台 Mac 吗？", answer: "不会。凭据仅在本机保存与使用；前端不会接触原始 token，日志也会对账号和认证信息脱敏。" },
      { question: "需要什么系统？", answer: "v1 面向 Apple Silicon Mac，建议 macOS 13 或更高版本。下载 DMG 后将应用拖入 Applications。" },
      { question: "为什么 macOS 显示开发者无法验证？", answer: "这是未公证版本的 Gatekeeper 提示。请在系统设置 → 隐私与安全性中允许打开，或右键应用选择打开。" },
    ],
  },
  en: {
    nav: { product: "Product", workflow: "How it works", security: "Privacy", faq: "Docs", github: "GitHub", download: "Download for macOS" },
    hero: {
      kicker: "LOCAL-FIRST MODEL ROUTER FOR CODEX",
      title: "Choose models from different providers, right inside ChatGPT.",
      description: "Connect Kimi, DeepSeek, Grok, OpenAI multi-account setups, or any compatible gateway. Configure once, then switch models from ChatGPT / Codex's native picker with one button.",
    },
    workflow: [
      { title: "Connect providers", description: "Use an official API, provider JSON, or multi-account credentials to bring your models together." },
      { title: "Enable models", description: "Discover models, then publish the ones you want in ChatGPT / Codex's native picker." },
      { title: "Choose in ChatGPT / Codex", description: "Press one button to move between providers and models without leaving your familiar workflow." },
    ],
    faq: [
      { question: "Does Codex Spur modify ChatGPT.app?", answer: "No. Spur integrates through a local Responses-compatible proxy, a model catalog, and the dedicated codex_select provider. It does not inject into or overwrite ChatGPT.app." },
      { question: "Do my credentials leave this Mac?", answer: "No. Credentials are stored and used locally. The frontend never receives raw tokens, and logs redact account and authentication data." },
      { question: "What do I need?", answer: "Version 1 targets Apple Silicon Macs running macOS 13 or later. Download the DMG and drag the app to Applications." },
      { question: "Why does macOS say the developer cannot be verified?", answer: "This is Gatekeeper's warning for an unsigned or non-notarized build. Open System Settings → Privacy & Security to allow it, or right-click the app and choose Open." },
    ],
  },
};

const providers = ["OpenAI", "Kimi", "DeepSeek", "Grok", "Anthropic", "Compatible gateways"];
const reasoning = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
const pickerModels = [
  "Grok · grok-4.5",
  "Kimi code · K2.7 Coding",
  "deepseek · deepseek-v4-flash",
  "OpenAI json · GPT-5.6-Sol",
];

function Arrow() { return <span aria-hidden="true">↗</span>; }

function useReveal() {
  useEffect(() => {
    const nodes = Array.from(document.querySelectorAll<HTMLElement>("[data-reveal]"));
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      nodes.forEach((node) => node.classList.add("is-visible"));
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          entry.target.classList.add("is-visible");
          observer.unobserve(entry.target);
        }
      });
    }, { threshold: 0.14 });
    nodes.forEach((node) => observer.observe(node));
    return () => observer.disconnect();
  }, []);
}

export function Site({ locale }: { locale: SiteLocale }) {
  useReveal();
  const t = copy[locale];
  const [menuOpen, setMenuOpen] = useState(false);
  const [openFaq, setOpenFaq] = useState<number | null>(null);
  const isZh = locale === "zh-CN";
  const langHref = isZh ? "/en/" : "/";

  return (
    <div className="site-shell">
      <header className="site-header">
        <a className="brand" href={isZh ? "/" : "/en/"} aria-label="Codex Spur home"><img src="/assets/codex-spur-icon.png" alt="" /><span>Codex Spur</span></a>
        <button className="menu-toggle" type="button" aria-expanded={menuOpen} aria-controls="site-nav" onClick={() => setMenuOpen((value) => !value)}><span>Menu</span><i /></button>
        <nav id="site-nav" className={menuOpen ? "site-nav site-nav--open" : "site-nav"}>
          <a href="#product" onClick={() => setMenuOpen(false)}>{t.nav.product}</a>
          <a href="#workflow" onClick={() => setMenuOpen(false)}>{t.nav.workflow}</a>
          <a href="#security" onClick={() => setMenuOpen(false)}>{t.nav.security}</a>
          <a href="#faq" onClick={() => setMenuOpen(false)}>{t.nav.faq}</a>
          <a href={release.releaseUrl} target="_blank" rel="noreferrer">{t.nav.github} <Arrow /></a>
          <a className="language-switch" href={langHref}>{isZh ? "中 / EN" : "ZH / EN"}</a>
          <a className="button button--dark button--small" href={release.assetUrl}>{t.nav.download} <Arrow /></a>
        </nav>
      </header>

      <main>
        <section className="hero section-pad" id="product">
          <div className="hero__copy" data-reveal>
            <p className="eyebrow"><span />{t.hero.kicker}</p>
            <h1>{t.hero.title}</h1>
            <p className="hero__lede">{t.hero.description}</p>
            <div className="hero__actions"><a className="button button--dark" href={release.assetUrl}>{isZh ? `下载 Codex Spur ${release.version.replace("v", "")}` : `Download Codex Spur ${release.version.replace("v", "")}`} <Arrow /></a><a className="text-link" href={release.releaseUrl} target="_blank" rel="noreferrer">{isZh ? "查看 GitHub" : "View on GitHub"} <Arrow /></a></div>
            <div className="trust-line"><span>● {isZh ? "密钥留在本机" : "Secrets stay local"}</span><span>● {isZh ? "不注入 ChatGPT.app" : "No ChatGPT.app injection"}</span><span>● Apple Silicon · MIT</span></div>
          </div>
          <HeroVisual locale={locale} />
        </section>

        <section className="choice-section section-pad" data-reveal>
          <div className="section-heading section-heading--split"><div><p className="eyebrow"><span />{isZh ? "ONE PICKER · MANY MODELS" : "ONE PICKER · MANY MODELS"}</p><h2>{isZh ? "一个选择器，调用不同厂商的模型。" : "One picker for models from different providers."}</h2></div><p>{isZh ? "真正需要切换的是模型，不是应用。" : "Switch the model, not the app."}</p></div>
          <PickerDemo locale={locale} />
        </section>

        <section className="capabilities section-pad" data-reveal>
          <div className="section-heading section-heading--split"><div><p className="eyebrow"><span />{isZh ? "CONTROL SURFACE, NOT A BLACK BOX" : "CONTROL SURFACE, NOT A BLACK BOX"}</p><h2>{isZh ? "每一个路由，都能对齐。" : "Every route stays aligned."}</h2></div><p>{isZh ? "模型、reasoning、账号池和额度状态，在同一个本地视图里互相对齐。" : "Models, reasoning, account pools, and quota states line up in one local view."}</p></div>
          <div className="capability-grid">
            <article className="capability-card capability-card--wide"><div><span className="card-index">01</span><h3>{isZh ? "多供应商，多个实例" : "Many providers. Many instances."}</h3><p>{isZh ? "OpenAI、Kimi、DeepSeek、Grok 或兼容网关可以并存；每个实例拥有独立配置与模型清单。" : "OpenAI, Kimi, DeepSeek, Grok, or compatible gateways can coexist with independent configuration and model lists."}</p></div><div className="provider-pills">{providers.map((provider) => <span key={provider}>{provider}</span>)}</div></article>
            <article className="capability-card"><span className="card-index">02</span><h3>{isZh ? "八档 reasoning" : "Eight reasoning levels"}</h3><p>{isZh ? "Codex 的 none 到 ultra 映射到真实上游行为，不夸大不可用的差异。" : "Codex's none-to-ultra ladder maps to real upstream behavior without pretending unsupported differences."}</p><div className="reasoning-list">{reasoning.map((level, index) => <span className={index === 4 ? "active" : ""} key={level}>{level}</span>)}</div></article>
            <article className="capability-card"><span className="card-index">03</span><h3>Pool / Fixed</h3><p>{isZh ? "按 previous response、会话和负载选择账号，也可以将路由固定到单个账号。" : "Select by previous response, session, and load—or pin a route to one account."}</p><div className="segmented"><span className="active">Pool</span><span>Fixed</span></div></article>
          </div>
        </section>

        <section className="workflow section-pad" id="workflow" data-reveal>
          <div className="section-heading"><p className="eyebrow"><span />{isZh ? "FROM PROVIDER TO PICKER" : "FROM PROVIDER TO PICKER"}</p><h2>{isZh ? "从接入到选择，只有三步。" : "From provider to picker in three steps."}</h2></div>
          <div className="workflow-grid">{t.workflow.map((step, index) => <article className="workflow-step" key={step.title}><span className="step-number">0{index + 1}</span><h3>{step.title}</h3><p>{step.description}</p>{index < 2 && <span className="step-arrow" aria-hidden="true">→</span>}</article>)}</div>
        </section>

        <section className="compatibility section-pad" data-reveal><div className="section-heading"><p className="eyebrow"><span />{isZh ? "COMPATIBLE BY DESIGN" : "COMPATIBLE BY DESIGN"}</p><h2>{isZh ? "从你已经在用的供应商开始。" : "Start with the providers you already use."}</h2></div><div className="compatibility-list">{providers.map((provider, index) => <div key={provider}><span>{String(index + 1).padStart(2, "0")}</span><strong>{provider}</strong><small>{index === 5 ? "Responses · Chat Completions · Anthropic" : index === 0 ? "Official · Responses" : "Provider instance"}</small><Arrow /></div>)}</div></section>

        <section className="security-band section-pad" id="security" data-reveal><div className="security-band__inner"><div className="security-band__intro"><p className="eyebrow eyebrow--light"><span />{isZh ? "LOCAL BY DEFAULT" : "LOCAL BY DEFAULT"}</p><h2>{isZh ? <>凭证留在本机，<br />选择器保持熟悉。</> : <>Credentials stay local.<br />The picker stays familiar.</>}</h2><p>{isZh ? "凭证、供应商和路由由本地桌面应用管理；ChatGPT / Codex 只看到稳定的模型入口。" : "Credentials, providers, and routes stay in the local desktop app. ChatGPT / Codex sees a stable model surface."}</p></div><div className="security-grid">{[["01 / LOCAL", isZh ? "密钥不离开 Mac" : "Secrets stay on Mac", isZh ? "本地加密保存，前端不接触原始密钥。" : "Encrypted local storage. The frontend never handles raw secrets."], ["02 / NATIVE", isZh ? "原生选择器切换" : "Switch in the native picker", isZh ? "不改客户端，不增加新的工作窗口。" : "No client modification. No new workflow window."], ["03 / READY", isZh ? "菜单栏代理常驻" : "A proxy that stays ready", isZh ? "关闭窗口继续路由，退出应用才释放租约。" : "Closing the window keeps routing alive; quitting releases leases."]].map(([eyebrow, title, body]) => <article className="security-card" key={eyebrow}><span>{eyebrow}</span><h3>{title}</h3><p>{body}</p></article>)}</div></div></section>

        <section className="faq section-pad" id="faq" data-reveal><div className="section-heading"><p className="eyebrow"><span />{isZh ? "BEFORE YOU DOWNLOAD" : "BEFORE YOU DOWNLOAD"}</p><h2>{isZh ? "开始之前，先知道这些。" : "A few things to know first."}</h2></div><div className="faq-list">{t.faq.map((item, index) => <div className={openFaq === index ? "faq-item faq-item--open" : "faq-item"} key={item.question}><button type="button" aria-expanded={openFaq === index} onClick={() => setOpenFaq(openFaq === index ? null : index)}><span>{item.question}</span><b>+</b></button>{openFaq === index && <p>{item.answer}</p>}</div>)}</div></section>

        <section className="download-cta section-pad"><div className="download-cta__copy"><p className="eyebrow"><span />{isZh ? "START LOCAL" : "START LOCAL"}</p><h2>{isZh ? "把模型选择，交还给你。" : "Put model choice back in your hands."}</h2><p>{isZh ? "Apple Silicon · macOS 13+ · MIT 开源许可" : "Apple Silicon · macOS 13+ · MIT licensed"}</p></div><div className="download-cta__actions"><a className="button button--dark" href={release.assetUrl}>{isZh ? "下载 macOS 版" : "Download for macOS"} <Arrow /></a><small>{isZh ? "仅支持 Apple Silicon · DMG" : "Apple Silicon only · DMG"}</small></div></section>
      </main>

      <footer className="site-footer section-pad"><a className="brand" href={isZh ? "/" : "/en/"}><img src="/assets/codex-spur-icon.png" alt="" /><span>Codex Spur</span></a><div><span>v0.1.0</span><a href={release.releaseUrl} target="_blank" rel="noreferrer">GitHub <Arrow /></a><a href={langHref}>{isZh ? "English" : "简体中文"}</a></div><p>Local-first model router for Codex.</p></footer>
    </div>
  );
}

function HeroVisual({ locale }: { locale: SiteLocale }) {
  const isZh = locale === "zh-CN";
  return <div className="hero-visual" data-reveal aria-label={isZh ? "ChatGPT Codex 模型选择器预览" : "ChatGPT Codex model picker preview"}><div className="hero-visual__grid" /><div className="app-window"><div className="window-top"><span /><span /><span /><b>Codex Spur</b><em>Overview</em></div><div className="app-body"><aside><strong>Codex Spur</strong><small>MODEL ROUTER</small><div className="active">▣ &nbsp; {isZh ? "概览" : "Overview"}</div><div>◇ &nbsp; {isZh ? "模型" : "Models"}</div><div>▥ &nbsp; {isZh ? "用量" : "Usage"}</div><div>⌁ &nbsp; {isZh ? "诊断" : "Diagnostics"}</div></aside><div className="app-content"><div className="app-content__label">CODEX SPUR</div><h3>{isZh ? "模型入口" : "Model surface"}</h3><div className="mini-panel"><b>{isZh ? "ChatGPT / Codex 原生选择器" : "ChatGPT / Codex native picker"}</b><span>{isZh ? "一个按钮 · 不同厂商 · 多个模型" : "One button · many providers · many models"}</span></div>{["OpenAI", "Grok", "Kimi code", "DeepSeek"].map((name) => <div className="provider-row" key={name}><i>{name[0]}</i><b>{name}</b><small>Responses · ready</small><span>✓</span></div>)}</div></div></div><div className="applied-pill"><i />{isZh ? "ChatGPT 模型选择器" : "ChatGPT model picker"}</div><div className="connector-line" /></div>;
}

function PickerDemo({ locale }: { locale: SiteLocale }) {
  const isZh = locale === "zh-CN";
  const [selected, setSelected] = useState(0);
  return <div className="picker-demo" data-reveal><div className="picker-demo__copy"><span className="card-index">CODEX / MODEL PICKER</span><h3>{isZh ? "一个按钮，调用不同厂商的模型。" : "One button. Models from different providers."}</h3><p>{isZh ? "在 ChatGPT 的模型选择页面里，直接选择你想用的模型。" : "Choose the model you want directly from ChatGPT's model picker."}</p><button className="button button--dark" type="button" onClick={() => setSelected((value) => (value + 1) % pickerModels.length)} aria-label={isZh ? "选择下一个模型" : "Choose the next model"}>{isZh ? "选择模型" : "Choose model"} <Arrow /></button><small>{isZh ? "当前选择" : "Selected"}: {pickerModels[selected]}</small></div><div className="picker-demo__panel"><div className="picker-demo__bar"><span>{isZh ? "模型" : "Model"}</span><b>⌄</b></div><div className="picker-demo__list" role="listbox" aria-label={isZh ? "模型选择器演示" : "Model picker demo"}>{pickerModels.map((model, index) => <button type="button" role="option" aria-selected={index === selected} className={index === selected ? "picker-option picker-option--selected" : "picker-option"} key={model} onClick={() => setSelected(index)}><i>{model[0]}</i><span>{model}</span>{index === selected && <b>✓</b>}</button>)}</div><div className="picker-demo__footer"><span>ChatGPT / Codex</span><span className="status-label"><i />{isZh ? "已连接" : "Connected"}</span></div></div></div>;
}
