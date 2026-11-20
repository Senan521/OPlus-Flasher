<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { makeT } from './i18n.js';
  import { Smartphone, Zap, Package, DatabaseBackup, Puzzle, FileUp, Settings, Usb, RefreshCw, FolderOpen, ShieldAlert, Power, KeyRound } from 'lucide-svelte';

  const SKIP = new Set(['userdata','metadata']);
  const tabs = [['device','Cihaz',Smartphone],['flash','Flaşla',Zap],['payload','Payload',Package],['extra','Extra',Puzzle],['settings','Ayarlar',Settings]];
  const titles = { device:'Cihaz', flash:'Flaşla', payload:'Payload', extra:'Extra', settings:'Ayarlar' };

  let tab = $state('device');
  let devices = $state([]), info = $state(null), adbInfo = $state(null), connName = $state('');
  let rootState = $state(''), shCmd = $state('getprop ro.build.fingerprint'), shOut = $state(''), fbCmd = $state('getvar all'), fbOut = $state(''), rawAsk = $state(false);
  let folder = $state('/storage/emulated/0/Download/ROM'), images = $state([]), picked = $state({}), progress = $state(0), current = $state('');
  let pPath = $state('/storage/emulated/0/Download/ota.zip'), pParts = $state([]), pPick = $state({}), pProg = $state(0), pCur = $state('');
  let bkDir = $state('/storage/emulated/0/LinuxFlasher/backups'), bkApps = $state([]), bkPick = $state({}), bkItems = $state([]), rPick = $state({}), bkProg = $state(0), bkCur = $state(''), bkStage = $state(''), bkQ = $state('');
  let sub = $state('general'), theme = $state('system'), autoConnect = $state(true);
  const store = { get(k, d) { try { return localStorage.getItem(k) ?? d; } catch { return d; } }, set(k, v) { try { localStorage.setItem(k, v); } catch {} } };
  function applyTheme() { const r = document.documentElement; if (theme === 'system') r.removeAttribute('data-theme'); else r.setAttribute('data-theme', theme); }
  function setLang(v) { lang = v; store.set('lf.lang', v); }
  function setTheme(v) { theme = v; store.set('lf.theme', v); applyTheme(); }
  const toggleAuto = () => { autoConnect = !autoConnect; store.set('lf.auto', autoConnect ? '1' : '0'); };
  const copyLog = async () => { try { await navigator.clipboard.writeText(logs.join('\n')); log(t('Günlük panoya kopyalandı')); } catch { log(`⚠ ${t('HATA')}: ${t('pano erişimi yok')}`); } };
  let flashSub = $state('images'), extraSub = $state('backup'), sdFile = $state(''), sdState = $state(''), sdProg = $state(0), ksuRepo = $state('tiann/KernelSU'), ksuStatus = $state(null), ksuRels = $state([]), ksuTag = $state(''), ksuKmi = $state(''), ksuPart = $state('init_boot');
  let ksuScan = $state(null), ksuOut = $state(null), ksuVb = $state(true), ksuReboot = $state(true), ksuLast = $state('');
  const sleep = (ms) => new Promise(r => setTimeout(r, ms));
  const sdCheck = async () => {
    if (!adbInfo) { rescan(); for (let i = 0; i < 60 && !adbInfo && (i < 8 || busy); i++) await sleep(500); }
    if (!adbInfo) { sdState = ''; log(`⚠ ${t('ADB cihazı bulunamadı')}`); return; }
    await guard('Sideload denetleniyor', async () => { sdState = await invoke('adb_state'); log(t('ADB durumu: {s}', { s: sdState })); }, 'device');
  };
  const sdRun = () => guard('Sideload gönderiliyor', async () => {
    sdProg = 0; log(`adb sideload ${sdFile}`);
    log(t(await invoke('adb_sideload', { path: sdFile }))); sdProg = 1;
  }, 'flash');
  const ksuInit = () => guard('KernelSU hazırlanıyor', async () => {
    ksuStatus = { ksud: true }; ksuScan = await invoke('ksu_scan', { folder }).catch(() => null); if (ksuScan?.recommended) ksuPart = ksuScan.recommended;
  }, 'extra');
  const ksuRel = () => guard('Sürümler alınıyor', async () => { ksuRels = await invoke('ksu_releases', { repo: ksuRepo }); if (ksuRels[0]) ksuTag = ksuRels[0].tag; log(t('{n} sürüm bulundu', { n: ksuRels.length })); }, 'extra');
  const ksuDetect = () => guard('KMI algılanıyor', async () => {
    const r = await invoke('ksu_detect_kmi', { folder }); if (r.kmi) { ksuKmi = r.kmi; log(t('KMI: {k} ({s})', { k: r.kmi, s: r.source })); } else log(t('KMI algılanamadı, elle seç'));
  }, 'extra');
  const ksuPatch = () => guard('Yamalanıyor', async () => { ksuOut = await invoke('ksu_patch', { repo: ksuRepo, tag: ksuTag, folder, partition: ksuPart, kmi: ksuKmi }); log(t('Yamalı imaj: {p}', { p: ksuOut.path })); }, 'extra');
  const ksuFlash = () => guard('KSU flaşlanıyor', async () => {
    const vb = ksuVb && ksuScan?.images?.find(i => i.name === 'vbmeta.img')?.path;
    log(await invoke('ksu_flash', { partition: ksuPart, file: ksuOut.path, vbmeta: vb || null, reboot: ksuReboot })); if (ksuReboot) { info = null; connName = ''; }
  }, 'extra');
  const ksuApk = (spoofed) => guard('Yönetici kuruluyor', async () => { log(await invoke('ksu_install_manager', { repo: ksuRepo, tag: ksuTag, spoofed })); }, 'extra');
  const ksuRoots = ['tiann/KernelSU', 'KernelSU-Next/KernelSU-Next', 'SukiSU-Ultra/SukiSU-Ultra', 'ReSukiSU/ReSukiSU'];
  const curRel = $derived(ksuRels.find(r => r.tag === ksuTag));
  let logs = $state([]), busy = $state(''), owner = $state('');
  let confirmOpen = $state(false), ack = $state(false), doneOpen = $state(false), wipeAck = $state(false);

  let lang = $state('system');
  const eff = $derived(lang === 'system' ? ((() => { const l = (navigator.language || 'en').toLowerCase(); return l.startsWith('tr') ? 'tr' : l.startsWith('ru') ? 'ru' : 'en'; })()) : lang);
  const { t, tr } = makeT(() => eff);
  let storageOk = $state(true);
  const checkStorage = async () => { try { storageOk = await invoke('storage_granted'); } catch {} };
  let bgOk = $state(true), keeps = 0;
  const checkBg = async () => { try { bgOk = await invoke('bg_granted'); } catch {} };
  const askBg = async () => { try { await invoke('bg_request'); } catch (e) { fail(e); } };
  const keep = (on) => {
    keeps += on ? 1 : -1;
    if (on && keeps === 1) invoke('keep_alive', { on: true }).catch(() => {});
    if (!on && keeps <= 0) { keeps = 0; invoke('keep_alive', { on: false }).catch(() => {}); }
  };
  const APP_NAMES = {
    'com.instagram.android':'Instagram','com.facebook.katana':'Facebook','com.facebook.orca':'Messenger','com.whatsapp':'WhatsApp','com.whatsapp.w4b':'WhatsApp Business',
    'org.telegram.messenger':'Telegram','org.thunderdog.challegram':'Telegram X','com.zhiliaoapp.musically':'TikTok','com.ss.android.ugc.trill':'TikTok','com.google.android.youtube':'YouTube',
    'com.google.android.apps.youtube.music':'YouTube Music','com.netflix.mediaclient':'Netflix','com.spotify.music':'Spotify','com.twitter.android':'X Twitter','com.snapchat.android':'Snapchat',
    'com.android.chrome':'Chrome','com.google.android.gm':'Gmail','com.google.android.apps.maps':'Maps Haritalar','com.google.android.apps.photos':'Photos Fotoğraflar','com.google.android.apps.docs':'Drive',
    'com.discord':'Discord','com.reddit.frontpage':'Reddit','com.pinterest':'Pinterest','com.linkedin.android':'LinkedIn','org.thoughtcrime.securesms':'Signal','com.viber.voip':'Viber',
    'com.skype.raider':'Skype','com.microsoft.teams':'Teams','com.amazon.mShop.android.shopping':'Amazon','com.ubercab':'Uber','tv.twitch.android.app':'Twitch','us.zoom.videomeetings':'Zoom',
    'org.videolan.vlc':'VLC','org.mozilla.firefox':'Firefox','com.topjohnwu.magisk':'Magisk','me.weishu.kernelsu':'KernelSU','com.termux':'Termux',
  };
  const appLabel = (p) => APP_NAMES[p] ?? p.split('.').slice(1).join(' ').replace(/\b\w/g, (c) => c.toUpperCase());
  const norm = (x) => x.normalize('NFD').replace(/[\u0300-\u036f]/g, '').toLowerCase().replaceAll('ı', 'i').replace(/[^a-z0-9]+/g, ' ').trim();
  const matchApp = (p, qq) => !qq || qq.split(' ').every((tok) => norm(`${p} ${appLabel(p)}`).includes(tok));
  const fail = (e) => {
    log(`⚠ ${t('HATA')}: ${tr(e)}`);
    if (/os error 13|Permission denied/i.test(String(e))) { storageOk = false; log(t('İpucu: Android “Tüm dosyalara erişim” iznini vermelisin (üstteki “İzin ver” kartı).')); checkStorage(); }
  };
  const log = (m) => { logs.push(`${new Date().toLocaleTimeString()}  ${m}`); if (logs.length > 500) logs.splice(0, logs.length - 500); };
  const fmt = (b) => b > 1e9 ? (b/1e9).toFixed(2)+' GB' : b > 1e6 ? (b/1e6).toFixed(1)+' MB' : (b/1e3).toFixed(0)+' KB';
  const slotted = $derived(info?.slotCount === '2');
  const names = $derived(images.filter(i => picked[i.name]).map(i => i.name));
  // Tek akış (Stock ROM + AOSP birleşik; klasörde olmayan imajlar zaten listelenmez/atlanır):
  // bootloader'da A/B, fastbootd'de yalnızca A, mantıksal bölümler silinip yeniden oluşturulur, sonda set_active a.
  const BL = ['boot', 'dtbo', 'init_boot', 'modem', 'recovery', 'vbmeta', 'vbmeta_system', 'vbmeta_vendor', 'vendor_boot'];
  const LOGICAL = ['my_bigball', 'my_carrier', 'my_engineering', 'my_heytap', 'my_manifest', 'my_product', 'my_region', 'my_stock', 'odm', 'product', 'system', 'system_dlkm', 'system_ext', 'vendor', 'vendor_dlkm', 'my_company', 'my_preload'];
  const place = (n) => BL.includes(n) ? 'bootloader · A/B' : LOGICAL.includes(n) ? 'fastbootd · mantıksal (A)' : 'fastbootd · A';
  const steps = $derived.by(() => {
    // oplusstanvbk ve my_region klasörde varsa özel bir şey gerekmez: ilki fastbootd'de _a, ikincisi mantıksal bölüm olarak yazılır.
    const bl = names.filter(n => BL.includes(n)), lo = LOGICAL.filter(n => names.includes(n)), fw = names.filter(n => !BL.includes(n) && !LOGICAL.includes(n));
    const NOSLOT = new Set(['userdata', 'metadata']); // A/B olmayan bölümler: _a eki yok
    return [
      ...bl.flatMap(n => [{ p: `${n}_a`, n }, { p: `${n}_b`, n }]),
      ...((fw.length || lo.length) && !info?.userspace ? [{ reboot: true }] : []),
      ...fw.map(n => ({ p: NOSLOT.has(n) ? n : `${n}_a`, n })),
      ...lo.flatMap(n => [{ logical: n }, { p: `${n}_a`, n }]),
      ...(names.length ? [{ active: 'a' }] : []),
    ];
  });
  const q = $derived(norm(bkQ));
  const bkView = $derived(bkApps.filter(p => matchApp(p, q)));
  const rView = $derived(bkItems.filter(i => matchApp(i.pkg, q)));
  const key0 = (x) => x;
  const allOn = (list, map, key = key0) => list.length > 0 && list.every(x => map[key(x)]);
  const countOn = (list, map, key = key0) => list.filter(x => map[key(x)]).length;
  function toggleAll(list, map, key = key0) { const on = !allOn(list, map, key); for (const x of list) map[key(x)] = on; }

  async function guard(label, fn, who = 'device') {
    busy = label; owner = who;
    const long = who !== 'device';
    // Ön plan servisi yalnızca 1,5 sn'den uzun işlerde başlatılır: çok kısa işlerde start+stop yarışı uygulamayı çökertiyordu.
    let started = false, kt = null;
    if (long) { kt = setTimeout(() => { started = true; keep(true); }, 1500); if (!bgOk) log(`⚠ ${t('Arka planda çalışma izni yok: uygulama arka plana alınırsa işlem durabilir.')}`); }
    try { return await fn(); } catch (e) { fail(e); } finally { busy = ''; owner = ''; if (long) { clearTimeout(kt); if (started) keep(false); } }
  }
  const pick = async (kind, cur) => { try { return await invoke(kind === 'dir' ? 'pick_folder' : 'pick_file'); } catch (e) { fail(e); return cur; } };

  // ---- otomatik algılama ----
  const tried = new Set();
  const supported = (d) => d.mode === 'fastboot' || d.mode === 'adb';
  function lost() {
    info = null; adbInfo = null; rootState = ''; connName = '';
    invoke('usb_disconnect'); invoke('adb_disconnect'); log(t('Cihaz bağlantısı kesildi'));
  }
  async function poll() {
    if (busy || document.hidden) return;
    let l; try { l = await invoke('usb_list'); } catch { return; }
    devices = l;
    const ids = new Set(l.map(d => d.name));
    for (const t of [...tried]) if (!ids.has(t)) tried.delete(t);
    if ((info || adbInfo) && !ids.has(connName)) return lost();
    if (!info && !adbInfo && autoConnect) {
      const d = l.find(x => supported(x) && !tried.has(x.name));
      if (d) { tried.add(d.name); log(t('USB cihazı algılandı: {p}', { p: d.product })); await connect(d); }
    }
  }
  const rescan = () => { tried.clear(); poll(); };

  const connect = (d) => guard('Bağlanılıyor', async () => {
    if (d.mode === 'adb') {
      log(t('Hedef cihazda “USB hata ayıklamaya izin ver” iletisi çıkarsa onayla'));
      adbInfo = await invoke('adb_connect', { name: d.name }); connName = d.name; log(t('ADB bağlandı: {m}', { m: adbInfo.model })); return;
    }
    info = await invoke('usb_connect', { name: d.name }); connName = d.name; log(t('Bağlandı: {p} · {s}', { p: info.product, s: info.serial }));
  });
  async function disconnect() { await invoke('usb_disconnect'); await invoke('adb_disconnect'); info = null; adbInfo = null; rootState = ''; connName = ''; }
  const reboot = (cmd) => guard(cmd, async () => {
    log(`> ${cmd}`); log((await invoke('fb_command', { cmd })) || 'OK'); info = null; connName = '';
  });
  const checkRoot = () => guard('Root denetleniyor', async () => { rootState = await invoke('adb_root_check'); log(t('Root: {r}', { r: rootState })); }, 'extra');
  const shRun = () => guard('Komut çalışıyor', async () => { log(`$ ${shCmd}`); const r = (await invoke('adb_exec', { cmd: shCmd })).trim() || t('(çıktı yok)'); shOut = r; log(r); });
  // Elle fastboot komutu (bootloader ve fastbootd). Kalıcı değişiklik yapabilenler önce onay ister.
  const RAW_DANGER = /^(fastboot\s+)?(erase|format|flashing|oem|delete-logical|create-logical|resize-logical|snapshot-update|set_active|--set-active)/i;
  const RAW_LEAVE = /^(fastboot\s+)?(reboot|continue)/i;
  const fbRun = (force = false) => {
    if (!fbCmd.trim()) return;
    if (!force && RAW_DANGER.test(fbCmd.trim())) { rawAsk = true; return; }
    rawAsk = false;
    return guard('Komut çalışıyor', async () => {
      log(`fastboot> ${fbCmd}`);
      const r = (await invoke('fb_raw', { cmd: fbCmd })).trim() || 'OK';
      fbOut = r; log(r);
      if (RAW_LEAVE.test(fbCmd.trim())) { info = null; connName = ''; }
    });
  };
  const adbReboot = (t) => guard('Yeniden başlatılıyor', async () => {
    await invoke('adb_reboot', { target: t }); log(`adb reboot ${t}`); adbInfo = null; rootState = ''; connName = '';
  });

  // ---- flaş ----
  const scan = () => guard('Taranıyor', async () => {
    images = await invoke('scan_images', { path: folder });
    picked = Object.fromEntries(images.map(i => [i.name, !SKIP.has(i.name)])); log(t('{n} imaj bulundu', { n: images.length }));
  }, 'flash');
  // Yeniden başlatılan cihaz birkaç saniye eski haliyle listede kalır; önce kaybolmasını bekle, sonra yeni cihaza bağlan.
  async function reconnect(old) {
    let gone = false;
    for (let i = 0; i < 60; i++) {
      await sleep(1000);
      try {
        const fb = (await invoke('usb_list')).find(x => x.mode === 'fastboot');
        if (!fb) { gone = true; continue; }
        if (!gone && fb.name === old) continue;
        const r = await invoke('usb_connect', { name: fb.name });
        if (!r.serial) continue; // cihaz henüz hazır değil
        info = r; connName = fb.name; return;
      } catch {}
    }
    throw 'Fastbootd cihazı bulunamadı';
  }
  async function run() {
    confirmOpen = false; ack = false;
    const ok = await guard('Flaşlanıyor', async () => {
      for (const s of steps) {
        if (s.reboot) { log(t('Fastbootd’a geçiliyor…')); const old = connName; await invoke('fb_command', { cmd: 'reboot-fastboot' }); await reconnect(old); log(t('Telefonda dil seçimi çıkarsa English seç')); continue; }
        if (s.logical) { current = s.logical; progress = 0; log(`logical ${s.logical}`); log(await invoke('fb_logical', { name: s.logical, path: `${folder}/${s.logical}.img` })); continue; }
        if (s.active) { current = ''; log(`set_active ${s.active}`); log(await invoke('fb_command', { cmd: `set_active:${s.active}` })); continue; }
        current = s.p; progress = 0; log(`flash ${s.p}`);
        log(await invoke('flash_image', { partition: s.p, path: `${folder}/${s.n}.img` }));
      }
      current = ''; log(t('Flaşlama tamamlandı.')); return true;
    }, 'flash');
    current = '';
    if (ok) doneOpen = true;
  }
  const wipe = () => guard('Veriler siliniyor', async () => { log(await invoke('fb_wipe')); doneOpen = false; wipeAck = false; }, 'flash');
  const rebootSys = () => { doneOpen = false; wipeAck = false; reboot('reboot'); };

  // ---- payload ----
  const pRead = () => guard('Payload okunuyor', async () => {
    pParts = await invoke('payload_list', { path: pPath }); pPick = Object.fromEntries(pParts.map(p => [p.name, true])); log(t('{n} bölüm bulundu', { n: pParts.length }));
  }, 'payload');
  async function pRun() {
    const ok = await guard('Çıkarılıyor', async () => {
      log(`${t('Çıktı: {p}', { p: '' })}${await invoke('payload_extract', { path: pPath, out: folder, names: pParts.filter(p => pPick[p.name]).map(p => p.name) })}`); return true;
    }, 'payload');
    if (ok) { await scan(); tab = 'flash'; }
  }

  // ---- yedek ----
  const bkLoad = () => guard('Uygulamalar okunuyor', async () => { bkApps = await invoke('bk_packages'); bkPick = {}; log(t('{n} kullanıcı uygulaması', { n: bkApps.length })); }, 'extra');
  const bkScan = () => guard('Yedekler okunuyor', async () => { bkItems = await invoke('bk_list', { out: bkDir }); rPick = {}; log(t('{n} yedek bulundu', { n: bkItems.length })); }, 'extra');
  // İşlem başlayınca ilerleme kartına kaydır (liste uzunsa kullanıcı göstergeyi göremiyordu).
  const toProgress = () => setTimeout(() => document.getElementById('bk-progress')?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 80);
  const bkRun = () => guard('Yedekleniyor', async () => {
    bkStage = ''; toProgress();
    await invoke('bk_backup', { pkgs: bkApps.filter(p => bkPick[p]), out: bkDir }); log(t('Yedekleme bitti')); bkItems = await invoke('bk_list', { out: bkDir });
  }, 'extra');
  let ssaidOn = $state(store.get('lf.ssaid', '0') === '1');
  const bkRestore = () => guard('Geri yükleniyor', async () => {
    bkStage = ''; toProgress();
    await invoke('bk_restore', { pkgs: bkItems.filter(i => rPick[i.pkg]).map(i => i.pkg), out: bkDir, ssaid: ssaidOn }); log(t('Geri yükleme bitti'));
  }, 'extra');

  onMount(() => {
    lang = store.get('lf.lang', 'system'); checkStorage();
    checkBg().then(() => { if (!bgOk && !store.get('lf.bgAsked', '')) { store.set('lf.bgAsked', '1'); askBg(); } });
    theme = store.get('lf.theme', 'system'); autoConnect = store.get('lf.auto', '1') === '1'; applyTheme();
    poll();
    const iv = setInterval(poll, 2000);
    const vis = () => { if (!document.hidden) { poll(); checkStorage(); checkBg(); } };
    document.addEventListener('visibilitychange', vis);
    const subs = [
      listen('sideload-progress', (e) => { const [d, t] = e.payload; sdProg = t ? d / t : 0; }),
      listen('flash-progress', (e) => { const [, d, t] = e.payload; progress = t ? d / t : 0; }),
      listen('payload-progress', (e) => { const [n, d, t] = e.payload; pCur = n; pProg = t ? d / t : 0; }),
      listen('bk-progress', (e) => { const [n, d, t] = e.payload; bkCur = n; bkProg = t ? d / t : 0; }),
      listen('adb-auth', () => log(t('Hedef cihazın ekranında USB hata ayıklama iznini onayla'))),
      listen('ksu-log', (e) => { ksuLast = e.payload; log(e.payload); }),
      listen('bk-log', (e) => { bkStage = e.payload; log(e.payload); }),
      listen('tauri://focus', checkStorage),
    ];
    return () => { clearInterval(iv); document.removeEventListener('visibilitychange', vis); subs.forEach(s => s.then(f => f())); };
  });
</script>

<div class="shell">
  <header class="appbar">
    <h1>{t(titles[tab])}</h1>
    <p>{info ? t('{p} bağlı', { p: info.product }) : adbInfo ? t('{m} bağlı (ADB)', { m: adbInfo.model }) : t('OTG ile hedef cihazı bağla')}</p>
  </header>

  <main><div class="col">
  {#if !storageOk}
    <section class="card tonal"><h2>{t('Dosya erişim izni gerekli')}</h2><p class="sub">{t('ROM, payload ve yedek dosyalarını okumak için “Tüm dosyalara erişim” iznini ver.')}</p>
      <button class="btn" onclick={() => invoke('storage_request')}>{t('İzin ver')}</button></section>
  {/if}
  {#if !bgOk}
    <section class="card tonal"><h2>{t('Arka planda çalışma izni gerekli')}</h2><p class="sub">{t('Flaşlama, yedekleme ve payload işlemleri uygulama arka plana alınınca durabilir. Pil optimizasyonunu kapat ve bildirimlere izin ver.')}</p>
      <button class="btn" onclick={askBg}>{t('İzin ver')}</button></section>
  {/if}
  {#if tab === 'device'}
    <section class="card hero">
      <div class="blob" class:live={info || adbInfo} class:busy={busy && owner === 'device'}><Usb size={32} /></div>
      <div>
        <h2>{info ? info.product || t('Fastboot cihazı') : adbInfo ? adbInfo.model : t('Cihaz bağlı değil')}</h2>
        <p class="sub">{busy && owner === 'device' ? t(busy) : info ? t('Seri: {s}', { s: info.serial }) : adbInfo ? t('Seri: {s}', { s: adbInfo.serial }) : t('Telefonu OTG kablosuyla tak, otomatik algılanır.')}</p>
      </div>
      {#if info || adbInfo}
        <button class="btn tonal sm" onclick={disconnect}>{t('Bağlantıyı kes')}</button>
      {:else}
        <button class="btn" onclick={rescan}><RefreshCw size={20} /> {t('Yeniden tara')}</button>
      {/if}
    </section>

    {#if adbInfo}
      <section class="card">
        {#if adbInfo.state === 'sideload'}
        <div class="chips"><span class="chip ok">ADB Sideload</span></div>
        <p class="sub">{t('Cihaz sideload modunda. Zip göndermek için Flaşla → ADB Sideload sekmesini aç.')}</p>
        {:else}
        <div class="chips"><span class="chip ok">ADB</span><span class="chip">{t('Android {v}', { v: adbInfo.android })}</span></div>
        <div class="kv"><div><small>{t('MODEL')}</small><b>{adbInfo.model}</b></div><div><small>{t('SERİ')}</small><b>{adbInfo.serial}</b></div></div>
        <div class="group">
          <button class="btn" onclick={() => adbReboot('')}>{t('Sistem')}</button><button class="btn" onclick={() => adbReboot('bootloader')}>Bootloader</button>
          <button class="btn" onclick={() => adbReboot('fastboot')}>Fastbootd</button><button class="btn" onclick={() => adbReboot('recovery')}>{t('Recovery')}</button>
        </div>
        <div class="field"><input id="sh" bind:value={shCmd} /><label for="sh">{t('ADB shell komutu')}</label></div>
        <button class="btn tonal sm" onclick={shRun} disabled={!!busy}>{t('Çalıştır')}</button>
        {#if shOut}<pre class="log out">{shOut}</pre>{/if}
        {/if}
      </section>
    {/if}
    {#if info}
      <section class="card">
        <div class="chips">
          <span class="chip {info.unlocked === 'yes' ? 'ok' : 'warn'}">{t('Bootloader {s}', { s: info.unlocked === 'yes' ? t('açık') : t('kilitli') })}</span>
          <span class="chip">{info.userspace ? 'Fastbootd' : 'Bootloader'}</span>
          {#if slotted}<span class="chip p">{t('Slot {s}', { s: info.currentSlot })}</span>{/if}
        </div>
        <div class="kv"><div><small>{t('ÜRÜN')}</small><b>{info.product || '—'}</b></div><div><small>{t('MAX İNDİRME')}</small><b>{fmt(info.maxDl)}</b></div></div>
        <div class="group">
          <button class="btn" onclick={() => reboot('reboot')}><Power size={18} /> {t('Sistem')}</button>
          <button class="btn" onclick={() => reboot('reboot-bootloader')}>Bootloader</button>
          <button class="btn" onclick={() => reboot('reboot-fastboot')}>Fastbootd</button>
        </div>
        <div class="field"><input id="fc" bind:value={fbCmd} autocapitalize="off" autocomplete="off" spellcheck="false" onkeydown={(e) => e.key === 'Enter' && fbRun()} /><label for="fc">{t('Fastboot komutu')}</label></div>
        <div class="chips">{#each ['getvar all', 'getvar current-slot', 'getvar is-userspace', 'oem device-info'] as c}<button class="chip" onclick={() => fbCmd = c}>{c}</button>{/each}</div>
        <button class="btn tonal sm" onclick={() => fbRun()} disabled={!!busy || !fbCmd.trim()}>{t('Çalıştır')}</button>
        {#if fbOut}<pre class="log out">{fbOut}</pre>{/if}
      </section>
    {/if}
    {#if !info && !adbInfo}
      {#each devices as d}
        <button class="row" onclick={() => connect(d)} disabled={!supported(d)} style="border:0;text-align:left;font:inherit;color:inherit">
          <Smartphone size={24} />
          <span class="grow"><b>{d.product}</b><small>{d.vid.toString(16)}:{d.pid.toString(16)} · {d.mode}</small></span>
          <span class="chip {supported(d) ? 'ok' : ''}">{supported(d) ? t('Bağlan') : t('Desteklenmiyor')}</span>
        </button>
      {/each}
    {/if}

  {:else if tab === 'flash'}
    <div class="group">
      <button class="btn" class:sel={flashSub === 'images'} onclick={() => flashSub = 'images'}>Fastboot Flasher</button>
      <button class="btn" class:sel={flashSub === 'sideload'} onclick={() => flashSub = 'sideload'}>{t('ADB Sideload')}</button>
    </div>
    {#if flashSub === 'sideload'}
      <section class="card">
        <div class="row" style="cursor:default"><FileUp size={26} /><span class="grow"><b>{t('ADB Sideload')}</b><small>{t('ROM veya başka bir zip dosyasını Recovery üzerinden yükle')}</small></span>
          <span class="chip {sdState === 'sideload' ? 'ok' : 'warn'}">{sdState === 'sideload' ? t('Sideload modunda') : sdState ? t('Sideload modunda değil ({s})', { s: sdState }) : t('Denetlenmedi')}</span></div>
        <p class="sub">{t('Hedef cihazda Recovery → Apply update → Apply from ADB seç, sonra aşağıdan denetle.')}</p>
        <button class="btn tonal" onclick={sdCheck} disabled={!!busy}>{t('Sideload modunu denetle')}</button>
        <button class="btn tonal" onclick={() => adbReboot('recovery')} disabled={!adbInfo || sdState === 'sideload' || !!busy}>{t('Recovery’ye yeniden başlat')}</button>
      </section>
      <section class="card">
        <div class="frow"><div class="field"><input id="sf" bind:value={sdFile} /><label for="sf">{t('Zip dosyası yolu')}</label></div>
          <button class="btn tonal" aria-label="Dosya seç" onclick={async () => sdFile = await pick('file', sdFile)}><FolderOpen size={22} /></button></div>
        <button class="btn" onclick={sdRun} disabled={!sdFile || sdState !== 'sideload' || !!busy}>{t('Sideload ile yükle')}</button>
        {#if sdState !== 'sideload'}<p class="sub">{t('Önce cihazın sideload modunda olduğunu denetle.')}</p>{/if}
        {#if busy === 'Sideload gönderiliyor' && owner === 'flash'}
          <div class="bar"><i style="width:{sdProg * 100}%"></i></div><p class="sub">%{Math.round(sdProg * 100)}</p>
          <button class="btn text sm" onclick={() => { invoke('sideload_cancel'); log(t('Durduruluyor…')); }}>{t('Durdur')}</button>
        {/if}
      </section>
    {:else}
    <section class="card">
      <div class="frow"><div class="field"><input id="f" bind:value={folder} /><label for="f">{t('ROM klasörü (.img dosyaları)')}</label></div>
        <button class="btn tonal" aria-label="Klasör seç" onclick={async () => folder = await pick('dir', folder)}><FolderOpen size={22} /></button></div>
      <button class="btn tonal" onclick={scan} disabled={!!busy}>{t('İmajları tara')}</button>
    </section>
    {#if busy === 'Flaşlanıyor' && owner === 'flash'}
      <section class="card tonal"><h2>{current || t('Hazırlanıyor')}</h2>
        <div class="bar"><i style="width:{progress * 100}%"></i></div><p class="sub">%{Math.round(progress * 100)}</p></section>
    {/if}
    {#if images.length}
      <div class="selbar"><span>{t('{n} imaj · {m} seçili', { n: images.length, m: names.length })}</span>
        <button class="btn text sm" onclick={() => toggleAll(images, picked, i => i.name)}>{allOn(images, picked, i => i.name) ? t('Seçimi kaldır') : t('Tümünü seç')}</button></div>
    {/if}
    {#each images as im}
      <div class="row" class:on={picked[im.name]} role="button" tabindex="0"
           onclick={() => picked[im.name] = !picked[im.name]} onkeydown={(e) => e.key === 'Enter' && (picked[im.name] = !picked[im.name])}>
        <span class="grow"><b>{im.name}</b><small>{fmt(im.size)}{place(im.name) ? ' · ' + t(place(im.name)) : ''}</small></span><span class="sw"></span>
      </div>
    {/each}
    {#if images.length}<div style="height:80px"></div>{/if}
    <button class="fab" disabled={!info || !steps.length || !!busy} onclick={() => confirmOpen = true}><Zap size={22} /> {t('Flaşla ({n})', { n: names.length })}</button>

    {/if}

  {:else if tab === 'payload'}
    <section class="card">
      <div class="row" style="cursor:default"><Package size={26} /><span class="grow"><b>Payload Dumper</b><small>{t('OTA zip dosyası veya Payload.bin dosyasını seçin')}</small></span></div>
      <div class="frow"><div class="field"><input id="pp" bind:value={pPath} /><label for="pp">{t('OTA zip / payload.bin yolu')}</label></div>
        <button class="btn tonal" aria-label="Dosya seç" onclick={async () => pPath = await pick('file', pPath)}><FolderOpen size={22} /></button></div>
      <p class="sub">{t('Çıktı klasörü: {f}', { f: folder })}</p>
      <button class="btn tonal" onclick={pRead} disabled={!!busy}>{t('Bölümleri oku')}</button>
      {#if busy === 'Çıkarılıyor' && owner === 'payload'}
        <div class="bar"><i style="width:{pProg * 100}%"></i></div><p class="sub">{pCur} · %{Math.round(pProg * 100)}</p>
        <button class="btn text sm" onclick={() => invoke('payload_cancel')}>{t('Durdur')}</button>
      {/if}
    </section>
    {#if pParts.length}
      <div class="selbar"><span>{t('{n} bölüm · {m} seçili', { n: pParts.length, m: countOn(pParts, pPick, p => p.name) })}</span>
        <button class="btn text sm" onclick={() => toggleAll(pParts, pPick, p => p.name)}>{allOn(pParts, pPick, p => p.name) ? t('Seçimi kaldır') : t('Tümünü seç')}</button></div>
    {/if}
    {#each pParts as p}
      <div class="row" class:on={pPick[p.name]} role="button" tabindex="0" onclick={() => pPick[p.name] = !pPick[p.name]} onkeydown={(e) => e.key === 'Enter' && (pPick[p.name] = !pPick[p.name])}>
        <span class="grow"><b>{p.name}</b><small>{fmt(p.size)}</small></span><span class="sw"></span></div>
    {/each}
    {#if pParts.length}<button class="btn wide" onclick={pRun} disabled={!!busy}>{t('Seçilenleri çıkar')}</button>{/if}

  {:else if tab === 'extra'}
    <div class="group">
      <button class="btn" class:sel={extraSub === 'backup'} onclick={() => extraSub = 'backup'}>{t('Yedek')}</button>
      <button class="btn" class:sel={extraSub === 'ksu'} onclick={() => { extraSub = 'ksu'; if (!ksuStatus) ksuInit(); }}>KernelSU</button>
    </div>
    {#if extraSub === 'ksu'}
      <section class="card">
        <div class="row" style="cursor:default"><KeyRound size={26} /><span class="grow"><b>{t('KernelSU yama')}</b><small>{t('ksud boot-patch · yalnızca GKI cihazlar')}</small></span>
          <span class="chip {adbInfo ? 'ok' : 'warn'}">{adbInfo ? t('ADB bağlı') : t('ADB gerekli')}</span></div>
        <p class="sub">{t('Yama hedef cihazda yapılır: hedef cihaz ADB ile bağlı olmalı (ksud cihaza gönderilip orada çalıştırılır).')}</p>
        <div class="field"><input id="kr" bind:value={ksuRepo} /><label for="kr">{t('GitHub deposu')}</label></div>
        <div class="chips">{#each ksuRoots as r}<button class="chip" onclick={() => ksuRepo = r}>{r.split('/')[1]}</button>{/each}</div>
        <button class="btn tonal" onclick={ksuRel} disabled={!!busy}>{t('Sürümleri getir')}</button>
      </section>
      {#each ksuRels as r}
        <div class="row" class:on={ksuTag === r.tag} role="button" tabindex="0" onclick={() => ksuTag = r.tag} onkeydown={(e) => e.key === 'Enter' && (ksuTag = r.tag)}>
          <span class="grow"><b>{r.tag}</b><small>{r.embedded ? t('KMI’ler ksud içinde') : t('{n} KMI', { n: r.kmis.length })}{r.prerelease ? ' · ' + t('ön sürüm') : ''}</small></span><span class="sw"></span></div>
      {/each}
      <section class="card">
        <div class="field"><input id="km" bind:value={ksuKmi} /><label for="km">{t('KMI (örn. android14-6.1)')}</label></div>
        <div class="chips">{#each (ksuRels.find(r => r.tag === ksuTag)?.kmis ?? []) as k}<button class="chip" onclick={() => ksuKmi = k}>{k}</button>{/each}</div>
        <button class="btn tonal sm" onclick={ksuDetect} disabled={!!busy}>{t('KMI’yi algıla')}</button>
        <div class="group"><button class="btn" class:sel={ksuPart === 'init_boot'} onclick={() => ksuPart = 'init_boot'}>init_boot</button>
          <button class="btn" class:sel={ksuPart === 'boot'} onclick={() => ksuPart = 'boot'}>boot</button></div>
        <p class="sub">{ksuScan?.images?.length ? t('Klasörde: {l}', { l: ksuScan.images.map(i => i.name).join(', ') }) : t('{f} içinde imaj yok', { f: folder })}</p>
        <button class="btn" onclick={ksuPatch} disabled={!!busy || !adbInfo || !ksuTag || !ksuKmi}>{t('Yamala')}</button>
        {#if busy === 'Yamalanıyor'}<p class="sub">{ksuLast}</p>{/if}
      </section>
      {#if ksuOut}
        <section class="card tonal"><h2>{t('Yamalı imaj hazır')}</h2><p class="sub" style="word-break:break-all">{ksuOut.path}</p>
          <div class="row" class:on={ksuVb} role="button" tabindex="0" onclick={() => ksuVb = !ksuVb} onkeydown={() => {}}><span class="grow"><b>{t('vbmeta doğrulamasını kapat')}</b><small>{t('klasörde vbmeta.img varsa')}</small></span><span class="sw"></span></div>
          <div class="row" class:on={ksuReboot} role="button" tabindex="0" onclick={() => ksuReboot = !ksuReboot} onkeydown={() => {}}><span class="grow"><b>{t('Sonra yeniden başlat')}</b></span><span class="sw"></span></div>
          {#if busy === 'KSU flaşlanıyor'}<div class="bar"><i style="width:{progress * 100}%"></i></div>{/if}
          <button class="btn" onclick={ksuFlash} disabled={!info || !!busy}>{t('Flaşla')}</button>
          {#if !info}<p class="sub">{t('Flaşlamak için cihaz fastboot modunda bağlı olmalı.')}</p>{/if}
        </section>
      {/if}
      {#if curRel}
        <section class="card">
          <h2>{t('Yönetici APK’sı')}</h2>
          <p class="sub">{t('Seçili sürümün yönetici uygulamasını hedef cihaza ADB ile kur.')}</p>
          <div class="group">
            <button class="btn tonal" onclick={() => ksuApk(false)} disabled={!adbInfo || !!busy || !curRel.apk}>{t('Normal sürüm')}</button>
            <button class="btn tonal" onclick={() => ksuApk(true)} disabled={!adbInfo || !!busy || !curRel.spoofName}>{t('Spoofed sürüm')}</button>
          </div>
          <p class="sub">{curRel.apkName ?? t('APK bulunamadı')}{curRel.spoofName ? ' · ' + curRel.spoofName : ' · ' + t('spoofed APK yok')}</p>
          {#if !adbInfo}<p class="sub">{t('Kurmak için hedef cihaz ADB ile bağlı olmalı.')}</p>{/if}
        </section>
      {/if}
    {:else}
    <section class="card">
      <div class="row" style="cursor:default"><DatabaseBackup size={26} /><span class="grow"><b>{t('Yedekle / Geri Yükle')}</b><small>{t('ADB Shell Root iznini verdiğinizden emin olun')}</small></span></div>
      <div class="frow"><div class="field"><input id="bo" bind:value={bkDir} /><label for="bo">{t('Yedek klasörü (bu telefonda)')}</label></div>
        <button class="btn tonal" aria-label="Klasör seç" onclick={async () => bkDir = await pick('dir', bkDir)}><FolderOpen size={22} /></button></div>
      <div class="group"><button class="btn" onclick={bkLoad} disabled={!adbInfo || !!busy}>{t('Uygulamalar')}</button><button class="btn" onclick={bkScan} disabled={!!busy}>{t('Yedekler')}</button></div>
      <div class="frow"><button class="btn tonal sm" style="flex:1" onclick={checkRoot} disabled={!adbInfo || !!busy}>{t('Root durumunu denetle')}</button>
        {#if rootState}<span class="chip ok">Root: {rootState}</span>{/if}</div>
      <div class="field"><input id="bq" bind:value={bkQ} /><label for="bq">{t('Paket adı ara')}</label></div>
      {#if !adbInfo}<p class="sub">{t('Yedeklemek için önce Cihaz sekmesinden ADB ile bağlan.')}</p>{/if}
      {#if (busy === 'Yedekleniyor' || busy === 'Geri yükleniyor') && owner === 'extra'}
        <div id="bk-progress" style="scroll-margin-top:72px"><p class="sub"><b>{t(busy)}</b></p>
        <div class="bar"><i style="width:{bkProg * 100}%"></i></div><p class="sub">{bkCur} · %{Math.round(bkProg * 100)}</p>
        {#if bkStage}<p class="sub">{bkStage}</p>{/if}</div>
        <button class="btn text sm" onclick={() => { invoke('bk_cancel'); log(t('Durduruluyor…')); }}>{t('Durdur')}</button>
      {/if}
    </section>
    {#if bkApps.length}
      <div class="selbar"><span>{t('Uygulamalar: {n} · {m} seçili', { n: bkView.length, m: countOn(bkApps, bkPick) })}</span>
        <button class="btn text sm" onclick={() => toggleAll(bkView, bkPick)}>{allOn(bkView, bkPick) ? t('Seçimi kaldır') : t('Tümünü seç')}</button></div>
    {/if}
    {#each bkView as p}
      <div class="row" class:on={bkPick[p]} role="button" tabindex="0" onclick={() => bkPick[p] = !bkPick[p]} onkeydown={(e) => e.key === 'Enter' && (bkPick[p] = !bkPick[p])}>
        <span class="grow"><b>{appLabel(p)}</b><small>{p}</small></span><span class="sw"></span></div>
    {/each}
    {#if bkApps.length}<button class="btn wide" onclick={bkRun} disabled={!!busy || !countOn(bkApps, bkPick)}>{t('Seçilenleri yedekle')}</button>{/if}
    {#if bkItems.length}
      <div class="selbar"><span>{t('Yedekler: {n} · {m} seçili', { n: rView.length, m: countOn(bkItems, rPick, i => i.pkg) })}</span>
        <button class="btn text sm" onclick={() => toggleAll(rView, rPick, i => i.pkg)}>{allOn(rView, rPick, i => i.pkg) ? t('Seçimi kaldır') : t('Tümünü seç')}</button></div>
    {/if}
    {#each rView as it}
      <div class="row" class:on={rPick[it.pkg]} role="button" tabindex="0" onclick={() => rPick[it.pkg] = !rPick[it.pkg]} onkeydown={(e) => e.key === 'Enter' && (rPick[it.pkg] = !rPick[it.pkg])}>
        <span class="grow"><b>{appLabel(it.pkg)}</b><small>{it.pkg} · {it.mb} MB · {new Date(it.time * 1000).toLocaleDateString()}</small></span><span class="sw"></span></div>
    {/each}
    {#if bkItems.length}
      <div class="row" class:on={ssaidOn} role="button" tabindex="0" onclick={() => { ssaidOn = !ssaidOn; store.set('lf.ssaid', ssaidOn ? '1' : '0'); }} onkeydown={(e) => e.key === 'Enter' && (ssaidOn = !ssaidOn, store.set('lf.ssaid', ssaidOn ? '1' : '0'))}>
        <span class="grow"><b>{t('SSAID’i de geri yükle')}</b><small>{t('Sistem dosyasını değiştirir (riskli); sonra cihazı yeniden başlat')}</small></span><span class="sw"></span></div>
      <button class="btn tonal wide" onclick={bkRestore} disabled={!adbInfo || !!busy || !countOn(bkItems, rPick, i => i.pkg)}>{t('Seçilenleri geri yükle')}</button>{/if}

    {/if}
  {:else}
    <div class="group">
      <button class="btn" class:sel={sub === 'general'} onclick={() => sub = 'general'}>{t('Genel')}</button>
      <button class="btn" class:sel={sub === 'log'} onclick={() => sub = 'log'}>{t('Günlük ({n})', { n: logs.length })}</button>
    </div>
    {#if sub === 'general'}
      <section class="card"><h2>{t('Dil')}</h2>
        <div class="group">{#each [['system','Sistem'],['tr','Türkçe'],['en','English'],['ru','Русский']] as [v, l]}
          <button class="btn" class:sel={lang === v} onclick={() => setLang(v)}>{t(l)}</button>{/each}</div>
      </section>
      <section class="card"><h2>{t('Görünüm')}</h2>
        <div class="group">{#each [['system','Sistem'],['light','Açık'],['dark','Koyu']] as [v, l]}
          <button class="btn" class:sel={theme === v} onclick={() => setTheme(v)}>{t(l)}</button>{/each}</div>
      </section>
      <section class="card"><h2>{t('Bağlantı')}</h2>
        <div class="row" class:on={autoConnect} role="button" tabindex="0" onclick={toggleAuto} onkeydown={(e) => e.key === 'Enter' && toggleAuto()}>
          <span class="grow"><b>{t('Otomatik bağlan')}</b><small>{t('Fastboot/ADB cihazı takılınca kendiliğinden bağlan')}</small></span><span class="sw"></span></div>
      </section>
      <section class="card"><h2>{t('Hakkında')}</h2><img src="/logo.png" alt="OPlus Flasher" width="72" height="72" style="border-radius:20px"><p class="sub">{t('OPlus Flasher 0.3 · Material 3 Expressive')}<br>{t('ADB anahtarı uygulama verisinde saklanır.')}</p></section>
    {:else}
      <div class="group"><button class="btn" onclick={copyLog}>{t('Kopyala')}</button><button class="btn" onclick={() => logs = []}>{t('Temizle')}</button></div>
      <div class="log">{#each [...logs].reverse() as l}<div class:err={l.includes('⚠')}>{l}</div>{:else}{t('Henüz kayıt yok.')}{/each}</div>
    {/if}
  {/if}
  </div></main>

  <nav>
    {#each tabs as [id, label, Ic]}
      <button class:on={tab === id} onclick={() => tab = id}><span class="ind"><Ic size={22} />{#if busy && owner === id}<i class="dot"></i>{/if}</span>{t(label)}</button>
    {/each}
  </nav>
</div>

{#if confirmOpen}
  <div class="scrim" role="presentation" onclick={() => confirmOpen = false}>
    <div class="dialog" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <ShieldAlert size={32} color="var(--err)" />
      <h2>{t('{n} bölüm flaşlanacak', { n: steps.filter(s => s.p || s.logical).length })}</h2>
      <p class="sub">{t('Yanlış imaj cihazı bozabilir. Kabloyu çıkarma, uygulamayı kapatma ve ekranı açık tut.')}</p>
      <p class="sub">{t('Mantıksal bölümler silinip yeniden oluşturulur; B slotundaki kopyaları boş kalır. İşlem sonunda aktif slot A yapılır.')}</p>
      <div class="row" class:on={ack} role="button" tabindex="0" onclick={() => ack = !ack} onkeydown={() => {}}>
        <span class="grow">{t('Yedeğim var, riski anlıyorum')}</span><span class="sw"></span></div>
      <div class="acts"><button class="btn text" onclick={() => confirmOpen = false}>{t('Vazgeç')}</button>
        <button class="btn danger" disabled={!ack} onclick={run}>{t('Başlat')}</button></div>
    </div>
  </div>
{/if}

{#if rawAsk}
  <div class="scrim" role="presentation" onclick={() => rawAsk = false}>
    <div class="dialog" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <ShieldAlert size={32} color="var(--err)" />
      <h2>{t('Komutu gönder?')}</h2>
      <p class="sub"><b>{fbCmd}</b></p>
      <p class="sub">{t('Bu komut cihazda kalıcı değişiklik yapabilir. Ne yaptığından eminsen gönder.')}</p>
      <div class="acts"><button class="btn text" onclick={() => rawAsk = false}>{t('Vazgeç')}</button>
        <button class="btn danger" onclick={() => fbRun(true)}>{t('Gönder')}</button></div>
    </div>
  </div>
{/if}

{#if doneOpen}
  <div class="scrim" role="presentation">
    <div class="dialog" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <h2>{t('Flaşlama tamamlandı')}</h2>
      <p class="sub">{t('Yeni bir ROM’a geçtiysen verileri silmen (format data) gerekebilir. Bu işlem dahili depolamadaki her şeyi siler.')}</p>
      <div class="row" class:on={wipeAck} role="button" tabindex="0" onclick={() => wipeAck = !wipeAck} onkeydown={() => {}}>
        <span class="grow">{t('Her şeyin silineceğini anlıyorum')}</span><span class="sw"></span></div>
      <div class="acts"><button class="btn text" onclick={() => doneOpen = false}>{t('Şimdilik değil')}</button>
        <button class="btn tonal" onclick={rebootSys}>{t('Yeniden başlat')}</button>
        <button class="btn danger" disabled={!wipeAck} onclick={wipe}>{t('Verileri sil')}</button></div>
    </div>
  </div>
{/if}
