// TuneBox (音盒) — Rust + Tauri 2 原生前端
const { core, event } = window.__TAURI__;
const invoke = (cmd, args) => core.invoke(cmd, args);

// 默认占位封面（内联极光声波 SVG）
const DEFAULT_COVER =
  "data:image/svg+xml;utf8," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0%" stop-color="#0f172a"/><stop offset="100%" stop-color="#1e1b4b"/></linearGradient></defs><rect width="120" height="120" fill="url(#g)"/><g stroke="#22d3ee" stroke-width="5" stroke-linecap="round"><line x1="38" y1="50" x2="38" y2="70"/><line x1="49" y1="40" x2="49" y2="80"/><line x1="60" y1="32" x2="60" y2="88"/><line x1="71" y1="43" x2="71" y2="77"/><line x1="82" y1="52" x2="82" y2="68"/></g></svg>`
  );

const PLATFORM_LABELS = {
  allmusic: "全平台",
  netease: "网易云",
  qq: "QQ音乐",
  kugou: "酷狗",
  kuwo: "酷我",
  bilibili: "B站",
  migu: "咪咕",
  youtube: "YouTube",
};

const state = {
  streamPort: 0,
  providers: [],
  settings: null,
  // 精选歌单状态
  exploreSource: "netease",
  exploreFilter: "",
  exploreOffset: 0,
  // 搜索状态
  searchSource: "allmusic",
  searchType: "track",
  searchKeywords: "",
  searchPage: 1,
  // 播放状态
  queue: [],
  currentIndex: -1,
  playMode: "loop", // loop | one | shuffle
  lyrics: [], // [{ time, text, ttext }]
  activeLyricIdx: -1,
  lyricWinVisible: false,
  // 当前打开的歌单详情
  activePlaylist: null,
};

// ========== 工具函数 ==========
function toast(msg) {
  const el = document.getElementById("toast");
  el.textContent = msg;
  el.classList.add("show");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => el.classList.remove("show"), 2400);
}

function coverUrl(url) {
  if (!url) return DEFAULT_COVER;
  if (url.startsWith("//")) url = "https:" + url;
  // YouTube 缩略图走本地流代理，确保挂梯子时正常显示
  if (
    state.streamPort &&
    (url.includes("ytimg.com") ||
      url.includes("ggpht.com") ||
      url.includes("hdslb.com"))
  ) {
    return `http://127.0.0.1:${state.streamPort}/img?url=${encodeURIComponent(url)}`;
  }
  return url;
}

function formatTime(sec) {
  if (!Number.isFinite(sec) || sec < 0) return "00:00";
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function esc(str) {
  return String(str ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// ========== 本地自建歌单与收藏歌单存储 ==========
function getObj(key, def) {
  try {
    const raw = localStorage.getItem(key);
    return raw ? JSON.parse(raw) : def;
  } catch {
    return def;
  }
}

function setObj(key, val) {
  localStorage.setItem(key, JSON.stringify(val));
}

function listMyPlaylists(type = "my") {
  const key = type === "favorite" ? "favoriteplayerlists" : "playerlists";
  const ids = getObj(key, []);
  return ids.map((id) => getObj(id, null)).filter((p) => p && p.info);
}

function createMyPlaylist(title, initialTrack = null) {
  const id = "myplaylist_" + Date.now() + Math.random().toString(36).slice(2, 6);
  const ids = getObj("playerlists", []);
  ids.push(id);
  setObj("playerlists", ids);
  const tracks = initialTrack ? [initialTrack] : [];
  const cover = initialTrack?.img_url || "";
  setObj(id, {
    is_mine: 1,
    info: { id, title, cover_img_url: cover, source_url: "" },
    tracks,
  });
  renderSidebarPlaylists();
  return id;
}

function addTrackToMyPlaylist(listId, track) {
  const pl = getObj(listId, null);
  if (!pl) return;
  pl.tracks = pl.tracks || [];
  if (!pl.tracks.some((t) => t.id === track.id)) {
    pl.tracks.unshift(track);
    if (track.img_url) pl.info.cover_img_url = track.img_url;
    setObj(listId, pl);
    toast(`已添加到「${pl.info.title}」`);
  } else {
    toast("歌曲已在该歌单中");
  }
}

function removeTrackFromMyPlaylist(listId, trackId) {
  const pl = getObj(listId, null);
  if (!pl) return;
  pl.tracks = (pl.tracks || []).filter((t) => t.id !== trackId);
  setObj(listId, pl);
}

function removeSavedPlaylist(listId, type = "my") {
  const key = type === "favorite" ? "favoriteplayerlists" : "playerlists";
  const ids = getObj(key, []).filter((x) => x !== listId);
  setObj(key, ids);
  localStorage.removeItem(listId);
  renderSidebarPlaylists();
}

function saveFavoritePlaylist(playlist) {
  const id = playlist.info.id;
  const ids = getObj("favoriteplayerlists", []);
  if (!ids.includes(id)) {
    ids.push(id);
    setObj("favoriteplayerlists", ids);
  }
  setObj(id, { is_fav: 1, info: playlist.info, tracks: playlist.tracks });
  renderSidebarPlaylists();
  toast("已收藏歌单");
}

function renderSidebarPlaylists() {
  const myBox = document.getElementById("my-playlists-nav");
  const favBox = document.getElementById("fav-playlists-nav");
  const myList = listMyPlaylists("my");
  const favList = listMyPlaylists("favorite");

  myBox.innerHTML = myList.length
    ? myList
        .map(
          (p) =>
            `<button class="nav-item" data-pl="${esc(p.info.id)}"><span class="nav-icon">♪</span><span>${esc(p.info.title)}</span></button>`
        )
        .join("")
    : `<div class="nav-section-title">暂无自建歌单</div>`;

  favBox.innerHTML = favList.length
    ? favList
        .map(
          (p) =>
            `<button class="nav-item" data-pl="${esc(p.info.id)}"><span class="nav-icon">★</span><span>${esc(p.info.title)}</span></button>`
        )
        .join("")
    : `<div class="nav-section-title">暂无收藏歌单</div>`;

  myBox.querySelectorAll("[data-pl]").forEach((btn) => {
    btn.onclick = () => openPlaylist(btn.dataset.pl);
  });
  favBox.querySelectorAll("[data-pl]").forEach((btn) => {
    btn.onclick = () => openPlaylist(btn.dataset.pl);
  });
}

// ========== 视图切换 ==========
function switchView(name) {
  document.querySelectorAll(".view").forEach((v) => v.classList.remove("active"));
  const target = document.getElementById(`view-${name}`);
  if (target) target.classList.add("active");
  document.querySelectorAll(".sidebar .nav-item[data-nav], .mobile-bottom-nav .m-nav-btn[data-nav]").forEach((b) => {
    b.classList.toggle("active", b.dataset.nav === name);
  });
  document.getElementById("content-scroll").scrollTop = 0;
}

// ========== 1. 精选歌单 ==========
function renderExplorePlatformTabs() {
  const wrap = document.getElementById("explore-platform-tabs");
  wrap.innerHTML = state.providers
    .map(
      (p) =>
        `<button class="tab-btn ${p.name === state.exploreSource ? "active" : ""}" data-src="${p.name}">${p.label}</button>`
    )
    .join("");
  wrap.querySelectorAll(".tab-btn").forEach((btn) => {
    btn.onclick = async () => {
      state.exploreSource = btn.dataset.src;
      state.exploreFilter = "";
      state.exploreOffset = 0;
      renderExplorePlatformTabs();
      await loadExploreFilters();
      await loadExplorePlaylists(false);
    };
  });
}

async function loadExploreFilters() {
  const wrap = document.getElementById("explore-filters");
  try {
    const f = await invoke("api_playlist_filters", { source: state.exploreSource });
    const list = f.recommend || [];
    wrap.innerHTML = list
      .map(
        (item) =>
          `<button class="chip ${item.id === state.exploreFilter ? "active" : ""}" data-fid="${esc(item.id)}">${esc(item.name)}</button>`
      )
      .join("");
    wrap.querySelectorAll(".chip").forEach((c) => {
      c.onclick = () => {
        state.exploreFilter = c.dataset.fid;
        state.exploreOffset = 0;
        wrap.querySelectorAll(".chip").forEach((x) => x.classList.remove("active"));
        c.classList.add("active");
        loadExplorePlaylists(false);
      };
    });
  } catch {
    wrap.innerHTML = "";
  }
}

async function loadExplorePlaylists(append = false) {
  const grid = document.getElementById("explore-grid");
  if (!append) {
    grid.innerHTML = `<div class="muted">加载中…</div>`;
  }
  try {
    const list = await invoke("api_show_playlist", {
      source: state.exploreSource,
      offset: state.exploreOffset,
      filterId: state.exploreFilter,
    });
    const html = list
      .map(
        (p) => `
      <div class="playlist-card" data-pid="${esc(p.id)}">
        <img class="playlist-card-cover" loading="lazy" src="${esc(coverUrl(p.cover_img_url))}" onerror="this.src='${DEFAULT_COVER}'" />
        <div class="playlist-card-title" title="${esc(p.title)}">${esc(p.title)}</div>
      </div>`
      )
      .join("");
    if (append) {
      grid.insertAdjacentHTML("beforeend", html);
    } else {
      grid.innerHTML = html || `<div class="muted">暂无歌单（若为 YouTube 请确认代理已连通）</div>`;
    }
    grid.querySelectorAll(".playlist-card").forEach((card) => {
      card.onclick = () => openPlaylist(card.dataset.pid);
    });
  } catch (e) {
    if (!append) {
      grid.innerHTML = `<div class="muted">加载失败：${esc(e)}</div>`;
    } else {
      toast("加载失败：" + e);
    }
  }
}

// ========== 2. 搜索 ==========
function renderSearchPlatformTabs() {
  const wrap = document.getElementById("search-platform-tabs");
  const all = [{ name: "allmusic", label: "聚合(国内)" }, ...state.providers];
  wrap.innerHTML = all
    .map(
      (p) =>
        `<button class="tab-btn ${p.name === state.searchSource ? "active" : ""}" data-src="${p.name}">${p.label}</button>`
    )
    .join("");
  wrap.querySelectorAll(".tab-btn").forEach((btn) => {
    btn.onclick = () => {
      state.searchSource = btn.dataset.src;
      state.searchPage = 1;
      renderSearchPlatformTabs();
      if (state.searchKeywords) runSearch(false);
    };
  });
}

async function runSearch(append = false) {
  const box = document.getElementById("search-results");
  const moreWrap = document.getElementById("search-more-wrap");
  if (!state.searchKeywords.trim()) return;
  if (!append) {
    box.innerHTML = `<div class="muted">正在搜索「${esc(state.searchKeywords)}」…</div>`;
    moreWrap.style.display = "none";
  }
  try {
    const res = await invoke("api_search", {
      source: state.searchSource,
      keywords: state.searchKeywords,
      page: state.searchPage,
      searchType: state.searchType,
    });
    if (state.searchType === "track") {
      if (!append) {
        state._searchTracks = res.tracks || [];
      } else {
        state._searchTracks = (state._searchTracks || []).concat(res.tracks || []);
      }
      box.innerHTML = renderTrackTable(state._searchTracks, { showSource: true });
      bindTrackTableEvents(box, state._searchTracks);
      moreWrap.style.display = (res.tracks || []).length >= 10 ? "flex" : "none";
    } else {
      const pls = res.playlists || [];
      const cards = pls
        .map(
          (p) => `
        <div class="playlist-card" data-pid="${esc(p.id)}">
          <img class="playlist-card-cover" loading="lazy" src="${esc(coverUrl(p.cover_img_url))}" onerror="this.src='${DEFAULT_COVER}'" />
          <div class="playlist-card-title">${esc(p.title)}</div>
        </div>`
        )
        .join("");
      if (!append) {
        box.innerHTML = `<div class="playlist-grid">${cards}</div>`;
      } else {
        box.querySelector(".playlist-grid")?.insertAdjacentHTML("beforeend", cards);
      }
      box.querySelectorAll(".playlist-card").forEach((c) => {
        c.onclick = () => openPlaylist(c.dataset.pid);
      });
      moreWrap.style.display = pls.length >= 10 ? "flex" : "none";
    }
  } catch (e) {
    box.innerHTML = `<div class="muted">搜索出错：${esc(e)}</div>`;
  }
}

// ========== 3. 歌单详情 ==========
async function openPlaylist(listId) {
  if (!listId) return;
  switchView("playlist");
  const box = document.getElementById("playlist-detail");
  box.innerHTML = `<div class="muted">正在加载歌单…</div>`;

  // 本地自建或收藏歌单
  const local = getObj(listId, null);
  if (local && local.info && (listId.startsWith("myplaylist_") || local.is_mine)) {
    state.activePlaylist = local;
    renderPlaylistDetail(local, { isMine: true, isFav: false });
    return;
  }

  try {
    const pl = await invoke("api_get_playlist", { listId });
    state.activePlaylist = pl;
    const isFav = getObj("favoriteplayerlists", []).includes(listId);
    renderPlaylistDetail(pl, { isMine: false, isFav });
  } catch (e) {
    // 若网络失败但本地有收藏副本，展示本地缓存
    if (local && local.info) {
      state.activePlaylist = local;
      renderPlaylistDetail(local, { isMine: false, isFav: true });
    } else {
      box.innerHTML = `<div class="muted">加载歌单失败：${esc(e)}</div>`;
    }
  }
}

function renderPlaylistDetail(pl, { isMine, isFav }) {
  const box = document.getElementById("playlist-detail");
  const tracks = pl.tracks || [];
  box.innerHTML = `
    <div class="playlist-header">
      <img class="playlist-header-cover" src="${esc(coverUrl(pl.info.cover_img_url))}" onerror="this.src='${DEFAULT_COVER}'" />
      <div class="playlist-header-info">
        <div>
          <div class="playlist-header-title">${esc(pl.info.title)}</div>
          <div class="muted">共 ${tracks.length} 首歌曲</div>
        </div>
        <div class="playlist-header-actions">
          <button class="btn-primary" id="pl-play-all">▶ 播放全部</button>
          <button class="btn-ghost" id="pl-add-queue">＋ 加入当前播放队列</button>
          ${
            isMine
              ? `<button class="btn-ghost" id="pl-delete-mine">删除此歌单</button>`
              : isFav
              ? `<button class="btn-ghost" id="pl-unfav">★ 取消收藏</button>`
              : `<button class="btn-ghost" id="pl-fav">☆ 收藏歌单</button>`
          }
        </div>
      </div>
    </div>
    ${renderTrackTable(tracks, { showSource: true, removableFromListId: isMine ? pl.info.id : null })}
  `;

  document.getElementById("pl-play-all").onclick = () => {
    if (!tracks.length) return;
    state.queue = [...tracks];
    playIndex(0);
    renderQueue();
  };
  document.getElementById("pl-add-queue").onclick = () => {
    let added = 0;
    tracks.forEach((t) => {
      if (!state.queue.some((q) => q.id === t.id)) {
        state.queue.push(t);
        added++;
      }
    });
    renderQueue();
    toast(`已添加 ${added} 首到播放队列`);
  };
  const favBtn = document.getElementById("pl-fav");
  if (favBtn) {
    favBtn.onclick = () => {
      saveFavoritePlaylist(pl);
      renderPlaylistDetail(pl, { isMine: false, isFav: true });
    };
  }
  const unfavBtn = document.getElementById("pl-unfav");
  if (unfavBtn) {
    unfavBtn.onclick = () => {
      removeSavedPlaylist(pl.info.id, "favorite");
      renderPlaylistDetail(pl, { isMine: false, isFav: false });
      toast("已取消收藏");
    };
  }
  const delBtn = document.getElementById("pl-delete-mine");
  if (delBtn) {
    delBtn.onclick = () => {
      removeSavedPlaylist(pl.info.id, "my");
      switchView("explore");
      toast("已删除歌单");
    };
  }

  bindTrackTableEvents(box, tracks, isMine ? pl.info.id : null);
}

function renderTrackTable(tracks, { showSource = true, removableFromListId = null } = {}) {
  if (!tracks || !tracks.length) {
    return `<div class="muted">暂无歌曲</div>`;
  }
  const curId = state.queue[state.currentIndex]?.id;
  return `
    <table class="track-table">
      <thead>
        <tr>
          <th style="width:42px">#</th>
          <th>歌曲</th>
          <th>歌手</th>
          <th>专辑</th>
          ${showSource ? `<th style="width:76px">来源</th>` : ""}
          <th style="width:90px">操作</th>
        </tr>
      </thead>
      <tbody>
        ${tracks
          .map(
            (t, idx) => `
          <tr class="track-row ${t.disabled ? "disabled" : ""} ${t.id === curId ? "playing" : ""}" data-idx="${idx}">
            <td>${idx + 1}</td>
            <td title="${esc(t.title)}">${esc(t.title)}</td>
            <td class="${t.artist_id ? "link-cell" : ""}" data-jump="${esc(t.artist_id || "")}" title="${esc(t.artist)}">${esc(t.artist)}</td>
            <td class="${t.album_id ? "link-cell" : ""}" data-jump="${esc(t.album_id || "")}" title="${esc(t.album)}">${esc(t.album)}</td>
            ${showSource ? `<td><span class="badge">${esc(PLATFORM_LABELS[t.source] || t.source)}</span></td>` : ""}
            <td class="row-ops">
              <button class="icon-btn op-add-my" data-idx="${idx}" title="收藏到自建歌单">＋</button>
              <button class="icon-btn op-next" data-idx="${idx}" title="下一首播放">⤵</button>
              ${
                removableFromListId
                  ? `<button class="icon-btn op-del" data-idx="${idx}" title="从歌单移除">✕</button>`
                  : ""
              }
            </td>
          </tr>`
          )
          .join("")}
      </tbody>
    </table>
  `;
}

function bindTrackTableEvents(container, tracks, removableFromListId = null) {
  container.querySelectorAll(".track-row").forEach((row) => {
    row.addEventListener("click", (ev) => {
      if (ev.target.closest(".row-ops") || ev.target.closest(".link-cell[data-jump!='']")) return;
      const idx = Number(row.dataset.idx);
      const track = tracks[idx];
      if (!track) return;
      // 点击单曲时将当前列表载入队列并立即播放
      state.queue = [...tracks];
      playIndex(idx);
      renderQueue();
    });
  });

  container.querySelectorAll(".link-cell").forEach((cell) => {
    const target = cell.dataset.jump;
    if (target) {
      cell.addEventListener("click", (ev) => {
        ev.stopPropagation();
        openPlaylist(target);
      });
    }
  });

  container.querySelectorAll(".op-add-my").forEach((btn) => {
    btn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      openAddToMyModal(tracks[Number(btn.dataset.idx)]);
    });
  });

  container.querySelectorAll(".op-next").forEach((btn) => {
    btn.addEventListener("click", (ev) => {
      ev.stopPropagation();
      const t = tracks[Number(btn.dataset.idx)];
      if (!t) return;
      if (state.currentIndex < 0) {
        state.queue = [t];
        playIndex(0);
      } else {
        state.queue.splice(state.currentIndex + 1, 0, t);
        toast("已设为下一首播放");
      }
      renderQueue();
    });
  });

  if (removableFromListId) {
    container.querySelectorAll(".op-del").forEach((btn) => {
      btn.addEventListener("click", (ev) => {
        ev.stopPropagation();
        const t = tracks[Number(btn.dataset.idx)];
        if (!t) return;
        removeTrackFromMyPlaylist(removableFromListId, t.id);
        openPlaylist(removableFromListId);
      });
    });
  }
}

// ========== 4. 播放器核心与歌词同步 ==========
const audio = document.getElementById("audio-el");

async function playIndex(idx) {
  if (!state.queue.length) return;
  if (idx < 0) idx = state.queue.length - 1;
  if (idx >= state.queue.length) idx = 0;
  state.currentIndex = idx;
  const track = state.queue[idx];
  if (!track) return;

  // 更新界面信息
  document.getElementById("player-cover").src = coverUrl(track.img_url);
  document.getElementById("player-title").textContent = track.title || "未知歌曲";
  document.getElementById("player-artist").textContent = track.artist || "";
  const badge = document.getElementById("player-badge");
  badge.style.display = "inline-block";
  badge.textContent = PLATFORM_LABELS[track.source] || track.source;

  document.querySelectorAll(".track-row").forEach((r) => {
    const i = Number(r.dataset.idx);
    r.classList.toggle("playing", state.queue[i]?.id === track.id);
  });
  renderQueue();

  invoke("api_update_tray", { title: track.title || "", artist: track.artist || "" }).catch(() => {});
  loadLyricForTrack(track);

  try {
    const sound = await invoke("api_bootstrap_track", { track });
    // 确保等待期间用户没有切歌
    if (state.queue[state.currentIndex]?.id !== track.id) return;
    if (sound.platform && sound.platform !== track.source) {
      badge.textContent = `${PLATFORM_LABELS[track.source] || track.source} → ${PLATFORM_LABELS[sound.platform] || sound.platform}`;
    }
    audio.src = sound.url;
    await audio.play();
    document.getElementById("btn-play").textContent = "⏸";
  } catch (e) {
    toast(`${track.title}: ${e}`);
    // 自动跳下一首（防止死循环：仅当队列大于 1 首时）
    if (state.queue.length > 1) {
      setTimeout(() => {
        if (state.queue[state.currentIndex]?.id === track.id) {
          playNext(true);
        }
      }, 1200);
    }
  }
}

function togglePlay() {
  if (!audio.src) {
    if (state.queue.length) playIndex(Math.max(0, state.currentIndex));
    return;
  }
  if (audio.paused) {
    audio.play().catch(() => {});
    document.getElementById("btn-play").textContent = "⏸";
  } else {
    audio.pause();
    document.getElementById("btn-play").textContent = "▶";
  }
}

function playNext(auto = false) {
  if (!state.queue.length) return;
  if (auto && state.playMode === "one") {
    audio.currentTime = 0;
    audio.play().catch(() => {});
    return;
  }
  if (state.playMode === "shuffle" && state.queue.length > 1) {
    let next = state.currentIndex;
    while (next === state.currentIndex) {
      next = Math.floor(Math.random() * state.queue.length);
    }
    playIndex(next);
  } else {
    playIndex(state.currentIndex + 1);
  }
}

function playPrev() {
  if (!state.queue.length) return;
  playIndex(state.currentIndex - 1);
}

// 解析 LRC 歌词（含双语翻译对齐）
function parseLrc(lrcText, tlrcText) {
  const timeRe = /\[(\d{1,3}):(\d{1,2})(?:\.(\d{1,3}))?\]/g;
  const parseMap = (raw) => {
    const map = new Map();
    if (!raw) return map;
    for (const line of raw.split(/\r?\n/)) {
      const times = [];
      let m;
      timeRe.lastIndex = 0;
      while ((m = timeRe.exec(line)) !== null) {
        const min = parseInt(m[1], 10);
        const sec = parseInt(m[2], 10);
        const frac = m[3] ? parseInt(m[3].padEnd(3, "0"), 10) : 0;
        times.push(min * 60 + sec + frac / 1000);
      }
      const text = line.replace(timeRe, "").trim();
      if (text) {
        times.forEach((t) => map.set(t.toFixed(2), text));
      }
    }
    return map;
  };

  const mainMap = parseMap(lrcText);
  const transMap = parseMap(tlrcText);
  const arr = [];
  for (const [k, text] of mainMap.entries()) {
    arr.push({
      time: parseFloat(k),
      text,
      ttext: transMap.get(k) || "",
    });
  }
  arr.sort((a, b) => a.time - b.time);
  return arr;
}

async function loadLyricForTrack(track) {
  const box = document.getElementById("lyric-box");
  box.innerHTML = `<div class="lyric-empty">加载歌词中…</div>`;
  state.lyrics = [];
  state.activeLyricIdx = -1;
  try {
    const res = await invoke("api_lyric", { track });
    state.lyrics = parseLrc(res.lyric, res.tlyric);
    if (!state.lyrics.length) {
      box.innerHTML = `<div class="lyric-empty">纯音乐或暂无歌词</div>`;
      return;
    }
    box.innerHTML = state.lyrics
      .map(
        (l, i) => `
      <div class="lyric-line" data-lidx="${i}">
        <div>${esc(l.text)}</div>
        ${l.ttext ? `<div class="lyric-tline">${esc(l.ttext)}</div>` : ""}
      </div>`
      )
      .join("");
    box.querySelectorAll(".lyric-line").forEach((el) => {
      el.onclick = () => {
        const i = Number(el.dataset.lidx);
        if (state.lyrics[i]) {
          audio.currentTime = state.lyrics[i].time;
        }
      };
    });
  } catch {
    box.innerHTML = `<div class="lyric-empty">暂无歌词</div>`;
  }
}

function syncLyric(currentTime) {
  if (!state.lyrics.length) return;
  let idx = -1;
  for (let i = 0; i < state.lyrics.length; i++) {
    if (currentTime + 0.15 >= state.lyrics[i].time) {
      idx = i;
    } else {
      break;
    }
  }
  if (idx === state.activeLyricIdx || idx < 0) return;
  state.activeLyricIdx = idx;

  const box = document.getElementById("lyric-box");
  box.querySelectorAll(".lyric-line").forEach((el, i) => {
    el.classList.toggle("active", i === idx);
  });
  const activeEl = box.querySelector(`.lyric-line[data-lidx="${idx}"]`);
  if (activeEl) {
    activeEl.scrollIntoView({ behavior: "smooth", block: "center" });
  }
  const cur = state.lyrics[idx];
  if (cur) {
    invoke("api_send_lyric", { line: cur.text, tline: cur.ttext || "" }).catch(() => {});
  }
}

function renderQueue() {
  document.getElementById("queue-count").textContent = String(state.queue.length);
  const box = document.getElementById("queue-list");
  box.innerHTML = state.queue
    .map(
      (t, i) => `
    <div class="queue-item ${i === state.currentIndex ? "active" : ""}" data-qidx="${i}">
      <div class="queue-item-meta">
        <span>${esc(t.title)}</span>
        <span class="queue-item-artist">${esc(t.artist)}</span>
      </div>
      <button class="icon-btn q-del" data-qidx="${i}" title="移除">✕</button>
    </div>`
    )
    .join("");
  box.querySelectorAll(".queue-item").forEach((el) => {
    el.onclick = (ev) => {
      if (ev.target.closest(".q-del")) return;
      playIndex(Number(el.dataset.qidx));
    };
  });
  box.querySelectorAll(".q-del").forEach((btn) => {
    btn.onclick = (ev) => {
      ev.stopPropagation();
      const i = Number(btn.dataset.qidx);
      state.queue.splice(i, 1);
      if (i < state.currentIndex) state.currentIndex--;
      renderQueue();
    };
  });
}

// ========== 5. 添加到自建歌单弹窗 ==========
function openAddToMyModal(track) {
  if (!track) return;
  const backdrop = document.getElementById("modal-backdrop");
  const body = document.getElementById("modal-body");
  const myList = listMyPlaylists("my");
  body.innerHTML = `
    <button class="btn-primary" id="modal-new-pl">＋ 新建歌单并加入</button>
    ${myList
      .map(
        (p) =>
          `<button class="btn-ghost" data-mid="${esc(p.info.id)}" style="text-align:left">♪ ${esc(p.info.title)} (${(p.tracks || []).length})</button>`
      )
      .join("")}
  `;
  backdrop.classList.remove("hidden");

  document.getElementById("modal-new-pl").onclick = () => {
    const name = prompt("请输入新歌单名称：", track.title);
    if (name && name.trim()) {
      createMyPlaylist(name.trim(), track);
      toast(`已创建歌单「${name.trim()}」`);
      backdrop.classList.add("hidden");
    }
  };
  body.querySelectorAll("[data-mid]").forEach((btn) => {
    btn.onclick = () => {
      addTrackToMyPlaylist(btn.dataset.mid, track);
      backdrop.classList.add("hidden");
    };
  });
}

// ========== 6. 设置表单读写 ==========
function fillSettingsForm() {
  const s = state.settings;
  if (!s) return;
  const setProxyGroup = (radioName, urlInputId, proxyObj) => {
    const mode = proxyObj?.mode || "system";
    const r = document.querySelector(`input[name="${radioName}"][value="${mode}"]`);
    if (r) r.checked = true;
    document.getElementById(urlInputId).value = proxyObj?.url || "";
  };
  setProxyGroup("overseas_mode", "overseas-proxy-url", s.overseas_proxy);
  setProxyGroup("domestic_mode", "domestic-proxy-url", s.domestic_proxy);
  document.getElementById("chk-auto-source").checked = !!s.auto_choose_source;
  document.getElementById("chk-global-shortcut").checked = !!s.global_shortcut;
  document.getElementById("chk-close-to-tray").checked = !!s.close_to_tray;
}

function readProxyGroup(radioName, urlInputId) {
  const mode = document.querySelector(`input[name="${radioName}"]:checked`)?.value || "system";
  const url = document.getElementById(urlInputId).value.trim();
  if (mode === "custom") {
    return { mode: "custom", url: url || "http://127.0.0.1:7890" };
  }
  return { mode };
}

// ========== 初始化与事件绑定 ==========
async function init() {
  const boot = await invoke("api_init");
  state.streamPort = boot.stream_port;
  state.providers = boot.providers;
  state.settings = boot.settings;

  renderSidebarPlaylists();
  renderExplorePlatformTabs();
  renderSearchPlatformTabs();
  fillSettingsForm();
  await loadExploreFilters();
  await loadExplorePlaylists(false);

  // 左侧导航与移动端底部导航切换
  document.querySelectorAll(".sidebar .nav-item[data-nav], .mobile-bottom-nav .m-nav-btn[data-nav]").forEach((btn) => {
    btn.onclick = () => switchView(btn.dataset.nav);
  });

  // 新建自建歌单
  document.getElementById("btn-new-playlist").onclick = () => {
    const title = prompt("请输入新歌单名称：");
    if (title && title.trim()) {
      const id = createMyPlaylist(title.trim());
      openPlaylist(id);
    }
  };

  // 精选歌单加载更多
  document.getElementById("btn-explore-more").onclick = () => {
    state.exploreOffset += 35;
    loadExplorePlaylists(true);
  };

  // 顶部搜索框
  document.getElementById("search-form").onsubmit = (ev) => {
    ev.preventDefault();
    const kw = document.getElementById("search-input").value.trim();
    if (!kw) return;
    state.searchKeywords = kw;
    state.searchPage = 1;
    switchView("search");
    runSearch(false);
  };

  // 搜索类型（单曲 / 歌单）切换
  document.querySelectorAll("[data-stype]").forEach((btn) => {
    btn.onclick = () => {
      document.querySelectorAll("[data-stype]").forEach((x) => x.classList.remove("active"));
      btn.classList.add("active");
      state.searchType = btn.dataset.stype;
      state.searchPage = 1;
      if (state.searchKeywords) runSearch(false);
    };
  });

  document.getElementById("btn-search-more").onclick = () => {
    state.searchPage += 1;
    runSearch(true);
  };

  // 链接打开歌单
  document.getElementById("btn-parse-url").onclick = async () => {
    const url = document.getElementById("input-parse-url").value.trim();
    if (!url) return;
    const pid = await invoke("api_parse_url", { url });
    if (pid) {
      openPlaylist(pid);
    } else {
      toast("未能识别该链接，请确认是否为支持的平台歌单/专辑/歌手链接");
    }
  };

  // 保存设置
  document.getElementById("btn-save-settings").onclick = async () => {
    const newSettings = {
      ...state.settings,
      overseas_proxy: readProxyGroup("overseas_mode", "overseas-proxy-url"),
      domestic_proxy: readProxyGroup("domestic_mode", "domestic-proxy-url"),
      auto_choose_source: document.getElementById("chk-auto-source").checked,
      global_shortcut: document.getElementById("chk-global-shortcut").checked,
      close_to_tray: document.getElementById("chk-close-to-tray").checked,
    };
    await invoke("api_save_settings", { newSettings });
    state.settings = newSettings;
    toast("设置已保存并立即生效");
  };

  // 备份导出 / 导入
  document.getElementById("btn-export-backup").onclick = () => {
    const data = {};
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      data[k] = localStorage.getItem(k);
    }
    const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `tunebox_backup_${new Date().toISOString().slice(0, 10)}.json`;
    a.click();
  };

  document.getElementById("input-import-backup").onchange = async (ev) => {
    const file = ev.target.files?.[0];
    if (!file) return;
    try {
      const text = await file.text();
      const obj = JSON.parse(text);
      Object.entries(obj).forEach(([k, v]) => {
        localStorage.setItem(k, typeof v === "string" ? v : JSON.stringify(v));
      });
      renderSidebarPlaylists();
      toast("歌单备份导入成功！");
    } catch (e) {
      toast("备份文件格式错误：" + e);
    }
  };

  // 主题切换 & 桌面歌词
  document.getElementById("btn-theme").onclick = () => {
    document.body.classList.toggle("light");
  };
  document.getElementById("btn-lyric-float").onclick = async () => {
    state.lyricWinVisible = !state.lyricWinVisible;
    await invoke("api_toggle_lyric_window", { show: state.lyricWinVisible });
  };

  // 右侧抽屉
  document.querySelectorAll(".drawer-tab").forEach((tab) => {
    tab.onclick = () => {
      document.querySelectorAll(".drawer-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      const which = tab.dataset.dtab;
      document.getElementById("drawer-lyric").classList.toggle("hidden", which !== "lyric");
      document.getElementById("drawer-queue").classList.toggle("hidden", which !== "queue");
    };
  });
  document.getElementById("btn-close-drawer").onclick = () => {
    document.getElementById("right-drawer").classList.add("collapsed");
  };
  document.getElementById("btn-toggle-drawer").onclick = () => {
    document.getElementById("right-drawer").classList.toggle("collapsed");
  };
  document.getElementById("btn-clear-queue").onclick = () => {
    state.queue = [];
    state.currentIndex = -1;
    renderQueue();
  };

  // 播放栏按钮
  document.getElementById("btn-play").onclick = togglePlay;
  document.getElementById("btn-prev").onclick = playPrev;
  document.getElementById("btn-next").onclick = () => playNext(false);
  document.getElementById("btn-mode").onclick = (ev) => {
    const order = ["loop", "one", "shuffle"];
    const icons = { loop: "🔁", one: "🔂", shuffle: "🔀" };
    const names = { loop: "列表循环", one: "单曲循环", shuffle: "随机播放" };
    state.playMode = order[(order.indexOf(state.playMode) + 1) % order.length];
    ev.currentTarget.textContent = icons[state.playMode];
    toast(names[state.playMode]);
  };
  document.getElementById("btn-add-to-my").onclick = () => {
    const cur = state.queue[state.currentIndex];
    if (cur) openAddToMyModal(cur);
  };
  document.getElementById("modal-close").onclick = () => {
    document.getElementById("modal-backdrop").classList.add("hidden");
  };

  // 音频事件
  const prog = document.getElementById("progress-range");
  let draggingProg = false;
  prog.addEventListener("input", () => {
    draggingProg = true;
  });
  prog.addEventListener("change", () => {
    if (Number.isFinite(audio.duration) && audio.duration > 0) {
      audio.currentTime = (Number(prog.value) / 1000) * audio.duration;
    }
    draggingProg = false;
  });

  audio.addEventListener("timeupdate", () => {
    document.getElementById("time-current").textContent = formatTime(audio.currentTime);
    document.getElementById("time-duration").textContent = formatTime(audio.duration);
    if (!draggingProg && Number.isFinite(audio.duration) && audio.duration > 0) {
      prog.value = String(Math.floor((audio.currentTime / audio.duration) * 1000));
    }
    syncLyric(audio.currentTime);
  });
  audio.addEventListener("ended", () => playNext(true));
  audio.addEventListener("play", () => {
    document.getElementById("btn-play").textContent = "⏸";
  });
  audio.addEventListener("pause", () => {
    document.getElementById("btn-play").textContent = "▶";
  });

  // 音量
  const volRange = document.getElementById("volume-range");
  audio.volume = 0.8;
  volRange.addEventListener("input", () => {
    audio.volume = Number(volRange.value) / 100;
  });

  // 监听来自托盘和全局快捷键的事件
  event.listen("global-shortcut", (ev) => {
    const act = ev.payload;
    if (act === "space") togglePlay();
    else if (act === "left") playPrev();
    else if (act === "right") playNext(false);
    else if (act === "toggle-lyric") {
      document.getElementById("btn-lyric-float").click();
    }
  });
}

window.addEventListener("DOMContentLoaded", init);
