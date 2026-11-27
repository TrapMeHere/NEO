// Language switcher for the bilingual NEO documentation site.
// The two books are served from /en/ and /zh/; this bar stays fixed in the top-right
// corner and links to the same page in the other language.
(function () {
  "use strict";

  function targetLang(path, lang) {
    var match = path.match(/^(.*\/)(en|zh)(\/.*)?$/);
    if (!match) {
      return null;
    }
    var base = match[1];
    var rest = match[3] || "/";
    return base + lang + rest;
  }

  function currentLang(path) {
    var match = path.match(/^(.*\/)(en|zh)(\/.*)?$/);
    return match ? match[2] : null;
  }

  function install() {
    var path = window.location.pathname;
    var current = currentLang(path);
    if (!current) {
      return;
    }

    var bar = document.createElement("nav");
    bar.id = "neo-lang-switch";
    bar.setAttribute("aria-label", "Language");

    var zh = document.createElement("a");
    zh.href = targetLang(path, "zh") || "zh/";
    zh.textContent = "\u4e2d\u6587";
    zh.setAttribute("hreflang", "zh-CN");
    if (current === "zh") {
      zh.className = "active";
      zh.setAttribute("aria-current", "true");
    }

    var en = document.createElement("a");
    en.href = targetLang(path, "en") || "en/";
    en.textContent = "English";
    en.setAttribute("hreflang", "en");
    if (current === "en") {
      en.className = "active";
      en.setAttribute("aria-current", "true");
    }

    var separator = document.createElement("span");
    separator.textContent = "|";
    separator.setAttribute("aria-hidden", "true");

    bar.appendChild(zh);
    bar.appendChild(separator);
    bar.appendChild(en);
    document.body.appendChild(bar);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", install);
  } else {
    install();
  }
})();
