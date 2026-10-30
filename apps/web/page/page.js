// The page /app/: a computer gets ChessPenguin (the WebAssembly program, loaded only
// then); a phone or a tablet gets the page about the apps (docs/design/web-version.md
// §3.6, decisions «Веб-версия»), with a quiet way to open the web version anyway.
//
//   ?phone=1  the device page on any device (checks, screenshots of the site);
//             ?phone=android, ?phone=ios: that variant of it
//   ?phone=0  the program on any device
//   ?lang=xx  the language (also of the program)
(function () {
  "use strict";

  var LANGS = [
    ["en", "English"], ["ru", "Русский"], ["fr", "Français"], ["de", "Deutsch"], ["es", "Español"],
    ["it", "Italiano"], ["pt", "Português"], ["uz", "Oʻzbekcha"], ["kk", "Қазақша"],
  ];
  // The engine the web version downloads for analysis (Stockfish 19, brotli).
  var ENGINE_MB = 78;
  var ANYWAY_KEY = "cp.web.phoneAnyway";
  var SITE = "https://chess.vrubel.xyz";

  var params = new URLSearchParams(location.search);
  var texts = window.CP_TEXTS || {};

  function pickLang() {
    var asked = (params.get("lang") || "").toLowerCase();
    if (texts[asked]) return asked;
    var prefs = navigator.languages || [navigator.language || ""];
    for (var i = 0; i < prefs.length; i++) {
      var code = String(prefs[i]).split(/[-_]/)[0].toLowerCase();
      if (texts[code]) return code;
    }
    return "en";
  }
  var lang = pickLang();

  function t(key, vars) {
    var s = (texts[lang] && texts[lang][key]) || (texts.en && texts.en[key]) || key;
    for (var k in vars || {}) s = s.split("{" + k + "}").join(vars[k]);
    return s;
  }

  function el(tag, attrs, children) {
    var e = document.createElement(tag);
    for (var a in attrs || {}) {
      if (a === "text") e.textContent = attrs[a];
      else if (a === "onclick") e.addEventListener("click", attrs[a]);
      else e.setAttribute(a, attrs[a]);
    }
    (children || []).forEach(function (c) { if (c) e.appendChild(c); });
    return e;
  }

  function storage(get, value) {
    try {
      if (get) return localStorage.getItem(ANYWAY_KEY);
      if (value === null) localStorage.removeItem(ANYWAY_KEY);
      else localStorage.setItem(ANYWAY_KEY, value);
    } catch (e) {}
    return null;
  }

  // ----- the program --------------------------------------------------------------

  function startApp() {
    document.documentElement.lang = lang;
    document.title = t("app_title");
    document.getElementById("cp-device").hidden = true;
    var status = document.getElementById("cp-status");
    status.textContent = t("app_loading");
    status.hidden = false;
    var failed = function (e) {
      console.error("ChessPenguin did not start:", e);
      document.getElementById("cp-app").hidden = true;
      status.textContent = t("app_failed");
      status.className = "failed";
      status.hidden = false;
    };
    import("./chesspenguin_web.js")
      .then(function (m) { return m.default().then(function () { return m; }); })
      .then(function (m) {
        document.getElementById("cp-app").hidden = false;
        return m.start("cp-canvas");
      })
      .then(function () {
        status.hidden = true;
        document.getElementById("cp-canvas").focus();
      })
      .catch(failed);
  }

  // ----- the page for phones and tablets ---------------------------------------------

  function shots(names) {
    return el("div", { class: "shots" }, names.map(function (n) {
      return el("img", { src: "../static/screenshots/" + n, alt: "", loading: "lazy" });
    }));
  }

  function shareButton() {
    var url = SITE + "/app/?lang=" + lang;
    var label = navigator.share ? t("app_send_link") : t("copy_link");
    var b = el("button", { class: "btn", type: "button", text: "⇪ " + label });
    b.addEventListener("click", function () {
      if (navigator.share) {
        navigator.share({ title: "ChessPenguin", url: url }).catch(function () {});
      } else if (navigator.clipboard) {
        navigator.clipboard.writeText(url).then(function () { b.textContent = t("link_copied"); });
      }
    });
    return b;
  }

  function anyway() {
    var size = ENGINE_MB + (lang === "ru" || lang === "kk" ? " МБ" : " MB");
    return el("div", null, [
      el("hr"),
      el("button", {
        class: "anyway", type: "button", text: t("app_open_anyway"),
        onclick: function () { storage(false, "1"); startApp(); },
      }),
      el("p", { class: "weak", text: t("app_open_anyway_note", { size: size }) }),
    ]);
  }

  function androidPart() {
    var v = CPDevice.androidVersion();
    if (v && v < 12) {
      return [el("p", { text: t("app_and_old", { version: String(v) }) }), shareButton()];
    }
    var hint = el("p", { class: "weak", text: t("next_android"), hidden: "" });
    var show = function () { hint.hidden = false; };
    return [
      el("a", { class: "btn", href: "/download/android-noengine", rel: "nofollow", onclick: show }, [
        document.createTextNode(t("btn_android")),
        el("small", { text: t("and_light_title") }),
      ]),
      el("p", null, [
        el("a", { class: "link", href: "/download/android", rel: "nofollow", onclick: show, text: t("and_full_title") + " ›" }),
      ]),
      el("p", { class: "weak", text: t("and_requires") }),
      hint,
      shots(["phone/01-home-360.webp", "phone/03-game-360.webp", "phone/04-engine-360.webp", "phone/08-review-360.webp"]),
      el("p", null, [el("a", { class: "link", href: "/" + (lang === "en" ? "" : lang + "/") + "#download-android", text: t("app_and_more") + " ›" })]),
    ];
  }

  function iosPart() {
    return [
      el("p", { class: "lead", text: t("app_ios_lead") }),
      el("p", { text: t("app_ios_now") }),
      shareButton(),
      el("p", { style: "margin-top:16px", text: t("app_ios_notify") }),
      el("div", { class: "contacts" }, [
        el("a", { class: "link", href: "mailto:vrubelroman@gmail.com?subject=ChessPenguin", text: "✉ " + t("email") }),
        el("a", { class: "link", href: "https://t.me/vrubelr", rel: "noopener", text: "✈ Telegram" }),
      ]),
      shots(["01-game-640.webp", "04-review-640.webp"]),
    ];
  }

  function showDevicePage(kind) {
    document.documentElement.lang = lang;
    document.title = t("app_phone_title");
    var page = document.getElementById("cp-device");
    page.textContent = "";
    var select = el("select", { "aria-label": t("language") }, LANGS.map(function (l) {
      var o = el("option", { value: l[0], text: l[1] });
      if (l[0] === lang) o.selected = true;
      return o;
    }));
    select.addEventListener("change", function () {
      lang = select.value;
      showDevicePage(kind);
    });
    page.appendChild(el("header", null, [
      el("span", { class: "brand" }, [el("img", { src: "icon-64.png", alt: "" }), document.createTextNode("ChessPenguin")]),
      select,
    ]));
    page.appendChild(el("h1", { text: t("app_phone_title") }));
    var body;
    if (kind === "android-phone" || kind === "android-tablet") {
      body = [el("p", { text: t("app_and_lead") })].concat(androidPart());
    } else if (kind === "iphone" || kind === "ipad") {
      body = iosPart();
    } else {
      body = [
        el("p", { text: t("app_phone_generic") }),
        el("a", { class: "btn", href: "/download/android-noengine", rel: "nofollow", text: t("btn_android") }),
        el("p"),
        shareButton(),
      ];
    }
    body.forEach(function (n) { page.appendChild(n); });
    page.appendChild(anyway());
    page.hidden = false;
  }

  // ----- which one --------------------------------------------------------------------

  var asked = params.get("phone");
  var kind = CPDevice.deviceClass();
  if (asked === "1") {
    showDevicePage(kind === "computer" ? "phone" : kind);
  } else if (asked === "android" || asked === "ios") {
    showDevicePage(asked === "android" ? "android-phone" : "iphone");
  } else if (asked === "0" || kind === "computer" || storage(true) === "1") {
    startApp();
  } else {
    showDevicePage(kind);
  }
})();
