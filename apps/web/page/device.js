// What device opened the page (docs/design/web-version.md §3.6.1; decisions «Веб-версия»:
// the web application is for computers only, phones and tablets get the app page).
// Decided by the page itself before anything large is loaded: a server cannot tell an
// iPad (it says "Macintosh") nor see the size of the screen.
//
// One function for the site and /app/, so that they never disagree whom to send where
// (the site takes this file over when the web version is published).
(function (global) {
  "use strict";

  // "computer" | "iphone" | "ipad" | "android-phone" | "android-tablet" | "phone" | "tablet"
  function deviceClass() {
    var ua = navigator.userAgent || "";
    var uad = navigator.userAgentData;
    var plat = (uad && uad.platform) || navigator.platform || "";
    var touch = navigator.maxTouchPoints > 0;
    // CSS pixels of the whole screen, not of the window: a narrow window of a computer
    // stays a computer; min() removes the orientation.
    var short = Math.min(screen.width, screen.height);
    var coarse = matchMedia("(pointer: coarse)").matches;
    var finePresent = matchMedia("(any-pointer: fine)").matches;

    // iPadOS (and an iPhone asking for the desktop site) says "Mac"; a Mac has no touch.
    var ios = /iPhone|iPod|iPad/.test(ua) || (/Mac/.test(plat) && navigator.maxTouchPoints > 1);
    if (ios) return short < 600 ? "iphone" : "ipad";
    if (/Android/i.test(ua)) return short < 600 ? "android-phone" : "android-tablet";
    if (uad && uad.mobile) return short < 600 ? "phone" : "tablet";
    if (/CrOS/.test(ua)) return "computer";
    // A phone or tablet asking for the desktop site (Chrome on large tablets does by
    // default): a touch screen and no fine pointer. A laptop with a touch screen has its
    // touchpad, so it stays a computer.
    if (touch && coarse && !finePresent) return short < 600 ? "phone" : "tablet";
    return "computer";
  }

  // The Android version of an Android device (0: not said).
  function androidVersion() {
    var m = /Android (\d+)/.exec(navigator.userAgent || "");
    return m ? Number(m[1]) : 0;
  }

  global.CPDevice = { deviceClass: deviceClass, androidVersion: androidVersion };
})(window);
