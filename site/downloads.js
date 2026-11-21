// Fills the download section from downloads/version.json, which the
// Pages workflow writes next to the files of the latest release. Without it
// the page keeps saying that there is nothing to download yet.
(function () {
  "use strict";

  var platforms = [
    { suffix: "-windows-x86_64.zip", label: "Windows", detail: "Windows 10 and 11, 64-bit", tried: true },
    { suffix: "-macos-arm64.tar.gz", label: "macOS", detail: "Mac with Apple silicon" },
    { suffix: "-linux-x86_64.tar.gz", label: "Linux", detail: "64-bit, glibc 2.35 or newer" },
    { suffix: "-android.apk", label: "Android", detail: "Android 8 or newer" },
    { suffix: "-web.zip", label: "Web client only", detail: "For a bridge already set up" },
    { suffix: "-source.tar.gz", label: "Source code", detail: "Of this version" }
  ];

  function size(bytes) {
    return (bytes / 1048576).toLocaleString("en-GB", { maximumFractionDigits: 1 }) + " MB";
  }

  function date(iso) {
    var d = new Date(iso + "T12:00:00Z");
    return isNaN(d) ? iso : d.toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" });
  }

  function guess() {
    var ua = navigator.userAgent;
    if (/Windows/.test(ua)) return "Windows";
    if (/Mac OS X/.test(ua) && !/iPhone|iPad/.test(ua)) return "macOS";
    if (/Android/.test(ua)) return "Android";
    if (/Linux/.test(ua)) return "Linux";
    return null;
  }

  function show(release) {
    var list = document.querySelector("[data-release-list]");
    var mine = guess();
    var shown = 0;
    platforms.forEach(function (p) {
      var file = (release.files || []).filter(function (f) {
        return f.name.slice(-p.suffix.length) === p.suffix;
      })[0];
      if (!file) return;
      var a = document.createElement("a");
      a.className = "dl" + (p.label === mine ? " mine" : "");
      a.href = "downloads/" + encodeURIComponent(file.name);
      a.setAttribute("download", "");
      var title = document.createElement("strong");
      title.textContent = p.label;
      var detail = document.createElement("span");
      detail.textContent = p.detail + " · " + size(file.size);
      a.appendChild(title);
      a.appendChild(detail);
      if (p.label === mine) {
        var tag = document.createElement("em");
        tag.textContent = "Your system";
        a.appendChild(tag);
      }
      list.appendChild(a);
      shown += 1;
    });
    if (!shown) return;
    list.hidden = false;
    document.querySelector("[data-release-notes]").hidden = false;
    document.querySelector("[data-release-intro]").textContent =
      "Leto " + release.version + ", published on " + date(release.date) +
      ". This is a development version: see above for what has been tried.";
    document.querySelector("[data-release-pill]").textContent =
      "In development · version " + release.version;
  }

  fetch("downloads/version.json", { cache: "no-cache" })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (release) { if (release) show(release); })
    .catch(function () {});
})();
