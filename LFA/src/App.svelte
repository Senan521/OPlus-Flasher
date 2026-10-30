<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { makeT } from './i18n.js';
  import { Smartphone, Zap, Package, DatabaseBackup, Settings, Usb, RefreshCw, FolderOpen, ShieldAlert, Power, KeyRound } from 'lucide-svelte';

  const CHAIN = new Set(['boot','init_boot','vendor_boot','dtbo','vbmeta','vbmeta_system','vbmeta_vendor','recovery','abl','xbl','xbl_config','tz','hyp','modem','dsp']);
  const SKIP = new Set(['userdata','metadata']);
  const tabs = [['device','Cihaz',Smartphone],['flash','Flaşla',Zap],['payload','Payload',Package],['backup','Yedek',DatabaseBackup],['settings','Ayarlar',Settings]];
  const titles = { device:'Cihaz', flash:'Flaşla', payload:'Payload', backup:'Yedekle', settings:'Ayarlar' };

  let tab = $state('device');
  let devices = $state([]), info = $state(null), adbInfo = $state(null), connName = $state('');
  let rootState = $state(''), shCmd = $state('getprop ro.build.fingerprint');
  let folder = $state('/storage/emulated/0/Download/ROM'), images = $state([]), picked = $state({}), progress = $state(0), current = $state('');
  let pPath = $state('/storage/emulated/0/Download/ota.zip'), pParts = $state([]), pPick = $state({}), pProg = $state(0), pCur = $state('');
  let bkDir = $state('/storage/emulated/0/LinuxFlasher/backups'), bkApps = $state([]), bkPick = $state({}), bkItems = $state([]), rPick = $state({}), bkProg = $state(0), bkCur = $state(''), bkQ = $state('');
  let sub = $state('general'), theme = $state('system'), autoConnect = $state(true);
  const store = { get(k, d) { try { return localStorage.getItem(k) ?? d; } catch { return d; } }, set(k, v) { try { localStorage.setItem(k, v); } catch {} } };
  function applyTheme() { const r = document.documentElement; if (theme === 'system') r.removeAttribute('data-theme'); else r.setAttribute('data-theme', theme); }
  function setLang(v) { lang = v; store.set('lf.lang', v); }
  function setTheme(v) { theme = v; store.set('lf.theme', v); applyTheme(); }
  const toggleAuto = () => { autoConnect = !autoConnect; store.set('lf.auto', autoConnect ? '1' : '0'); };
  const copyLog = async () => { try { await navigator.clipboard.writeText(logs.join('\n')); log(t('Günlük panoya kopyalandı')); } catch { log(`⚠ ${t('HATA')}: ${t('pano erişimi yok')}`); } };
  let flashSub = $state('images'), ksuRepo = $state('tiann/KernelSU'), ksuStatus = $state(null), ksuRels = $state([]), ksuTag = $state(''), ksuKmi = $state(''), ksuPart = $state('init_boot');
  let ksuScan = $state(null), ksuOut = $state(null), ksuVb = $state(true), ksuReboot = $state(true), ksuLast = $state('');
  const ksuInit = () => guard('KernelSU hazırlanıyor', async () => {
    ksuStatus = { ksud: true }; ksuScan = await invoke('ksu_scan', { folder }).catch(() => null); if (ksuScan?.recommended) ksuPart = ksuScan.recommended;
  }, 'flash');
  const ksuRel = () => guard('Sürümler alınıyor', async () => { ksuRels = await invoke('ksu_releases', { repo: ksuRepo }); if (ksuRels[0]) ksuTag = ksuRels[0].tag; log(t('{n} sürüm bulundu', { n: ksuRels.length })); }, 'flash');
  const ksuDetect = () => guard('KMI algılanıyor', async () => {
    const r = await invoke('ksu_detect_kmi', { folder }); if (r.kmi) { ksuKmi = r.kmi; log(t('KMI: {k} ({s})', { k: r.kmi, s: r.source })); } else log(t('KMI algılanamadı, elle seç'));
  }, 'flash');
  const ksuPatch = () => guard('Yamalanıyor', async () => { ksuOut = await invoke('ksu_patch', { repo: ksuRepo, tag: ksuTag, folder, partition: ksuPart, kmi: ksuKmi }); log(t('Yamalı imaj: {p}', { p: ksuOut.path })); }, 'flash');
  const ksuFlash = () => guard('KSU flaşlanıyor', async () => {
    const vb = ksuVb && ksuScan?.images?.find(i => i.name === 'vbmeta.img')?.path;
    log(await invoke('ksu_flash', { partition: ksuPart, file: ksuOut.path, vbmeta: vb || null, reboot: ksuReboot })); if (ksuReboot) { info = null; connName = ''; }
  }, 'flash');
  const ksuApk = () => guard('Yönetici kuruluyor', async () => { log(await invoke('ksu_install_manager', { repo: ksuRepo, tag: ksuTag, spoofed: false })); }, 'flash');
  let logs = $state([]), busy = $state(''), owner = $state('');
  let confirmOpen = $state(false), ack = $state(false), doneOpen = $state(false), wipeAck = $state(false);

  let lang = $state('system');
  const eff = $derived(lang === 'system' ? ((navigator.language || 'en').toLowerCase().startsWith('tr') ? 'tr' : 'en') : lang);
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
  const steps = $derived.by(() => {
    const t = (n) => (slotted ? [`${n}_a`, `${n}_b`] : [n]).map(p => ({ p, n }));
    const c = names.filter(n => CHAIN.has(n)), r = names.filter(n => !CHAIN.has(n));
    return [...c.flatMap(t), ...(r.length && !info?.userspace ? [{ reboot: true }] : []), ...r.flatMap(t)];
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
    if (long) { keep(true); if (!bgOk) log(`⚠ ${t('Arka planda çalışma izni yok: uygulama arka plana alınırsa işlem durabilir.')}`); }
    try { return await fn(); } catch (e) { fail(e); } finally { busy = ''; owner = ''; if (long) keep(false); }
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
  const checkRoot = () => guard('Root denetleniyor', async () => { rootState = await invoke('adb_root_check'); log(t('Root: {r}', { r: rootState })); }, 'backup');
  const shRun = () => guard('Komut çalışıyor', async () => { log(`$ ${shCmd}`); log((await invoke('adb_exec', { cmd: shCmd })).trim() || t('(çıktı yok)')); });
  const adbReboot = (t) => guard('Yeniden başlatılıyor', async () => {
    await invoke('adb_reboot', { target: t }); log(`adb reboot ${t}`); adbInfo = null; rootState = ''; connName = '';
  });

  // ---- flaş ----
  const scan = () => guard('Taranıyor', async () => {
    images = await invoke('scan_images', { path: folder });
    picked = Object.fromEntries(images.map(i => [i.name, !SKIP.has(i.name)])); log(t('{n} imaj bulundu', { n: images.length }));
  }, 'flash');
  async function reconnect() {
    for (let i = 0; i < 40; i++) {
      await new Promise(r => setTimeout(r, 1000));
      try {
        const d = (await invoke('usb_list')).find(x => x.mode === 'fastboot');
        if (d) { info = await invoke('usb_connect', { name: d.name }); connName = d.name; return; }
      } catch {}
    }
    throw 'Fastbootd cihazı bulunamadı';
  }
  async function run() {
    confirmOpen = false; ack = false;
    const ok = await guard('Flaşlanıyor', async () => {
      for (const s of steps) {
        if (s.reboot) { log(t('Fastbootd’a geçiliyor…')); await invoke('fb_command', { cmd: 'reboot-fastboot' }); await reconnect(); continue; }
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
  const bkLoad = () => guard('Uygulamalar okunuyor', async () => { bkApps = await invoke('bk_packages'); bkPick = {}; log(t('{n} kullanıcı uygulaması', { n: bkApps.length })); }, 'backup');
  const bkScan = () => guard('Yedekler okunuyor', async () => { bkItems = await invoke('bk_list', { out: bkDir }); rPick = {}; log(t('{n} yedek bulundu', { n: bkItems.length })); }, 'backup');
  const bkRun = () => guard('Yedekleniyor', async () => {
    await invoke('bk_backup', { pkgs: bkApps.filter(p => bkPick[p]), out: bkDir }); log(t('Yedekleme bitti')); bkItems = await invoke('bk_list', { out: bkDir });
  }, 'backup');
  const bkRestore = () => guard('Geri yükleniyor', async () => {
    await invoke('bk_restore', { pkgs: bkItems.filter(i => rPick[i.pkg]).map(i => i.pkg), out: bkDir }); log(t('Geri yükleme bitti'));
  }, 'backup');

  onMount(() => {
    lang = store.get('lf.lang', 'system'); checkStorage();
    checkBg().then(() => { if (!bgOk && !store.get('lf.bgAsked', '')) { store.set('lf.bgAsked', '1'); askBg(); } });
    theme = store.get('lf.theme', 'system'); autoConnect = store.get('lf.auto', '1') === '1'; applyTheme();
    poll();
    const iv = setInterval(poll, 2000);
    const vis = () => { if (!document.hidden) { poll(); checkStorage(); checkBg(); } };
    document.addEventListener('visibilitychange', vis);
    const subs = [
      listen('flash-progress', (e) => { const [, d, t] = e.payload; progress = t ? d / t : 0; }),
      listen('payload-progress', (e) => { const [n, d, t] = e.payload; pCur = n; pProg = t ? d / t : 0; }),
      listen('bk-progress', (e) => { const [n, d, t] = e.payload; bkCur = n; bkProg = t ? d / t : 0; }),
      listen('adb-auth', () => log(t('Hedef cihazın ekranında USB hata ayıklama iznini onayla'))),
      listen('ksu-log', (e) => { ksuLast = e.payload; log(e.payload); }),
      listen('bk-log', (e) => log(e.payload)),
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
        <div class="chips"><span class="chip ok">ADB</span><span class="chip">{t('Android {v}', { v: adbInfo.android })}</span></div>
        <div class="kv"><div><small>{t('MODEL')}</small><b>{adbInfo.model}</b></div><div><small>{t('SERİ')}</small><b>{adbInfo.serial}</b></div></div>
        <div class="group">
          <button class="btn" onclick={() => adbReboot('')}>{t('Sistem')}</button><button class="btn" onclick={() => adbReboot('bootloader')}>Bootloader</button>
          <button class="btn" onclick={() => adbReboot('fastboot')}>Fastbootd</button><button class="btn" onclick={() => adbReboot('recovery')}>{t('Recovery')}</button>
        </div>
        <div class="field"><input id="sh" bind:value={shCmd} /><label for="sh">{t('ADB shell komutu')}</label></div>
        <button class="btn tonal sm" onclick={shRun} disabled={!!busy}>{t('Çalıştır')}</button>
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
      <button class="btn" class:sel={flashSub === 'images'} onclick={() => flashSub = 'images'}>{t('İmajlar')}</button>
      <button class="btn" class:sel={flashSub === 'ksu'} onclick={() => { flashSub = 'ksu'; if (!ksuStatus) ksuInit(); }}>KernelSU</button>
    </div>
    {#if flashSub === 'ksu'}
      <section class="card">
        <div class="row" style="cursor:default"><KeyRound size={26} /><span class="grow"><b>{t('KernelSU yama')}</b><small>{t('ksud boot-patch · yalnızca GKI cihazlar')}</small></span>
          <span class="chip {adbInfo ? 'ok' : 'warn'}">{adbInfo ? t('ADB bağlı') : t('ADB gerekli')}</span></div>
        <p class="sub">{t('Yama hedef cihazda yapılır: hedef cihaz ADB ile bağlı olmalı (ksud cihaza gönderilip orada çalıştırılır).')}</p>
        <div class="field"><input id="kr" bind:value={ksuRepo} /><label for="kr">{t('GitHub deposu')}</label></div>
        <button class="btn tonal" onclick={ksuRel} disabled={!!busy}>{t('Sürümleri getir')}</button>
      </section>
      {#each ksuRels as r}
        <div class="row" class:on={ksuTag === r.tag} role="button" tabindex="0" onclick={() => ksuTag = r.tag} onkeydown={(e) => e.key === 'Enter' && (ksuTag = r.tag)}>
          <span class="grow"><b>{r.tag}</b><small>{t('{n} KMI', { n: r.kmis.length })}{r.prerelease ? ' · ' + t('ön sürüm') : ''}</small></span><span class="sw"></span></div>
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
          <button class="btn tonal" onclick={ksuApk} disabled={!adbInfo || !!busy}>{t('Yöneticiyi kur (ADB)')}</button>
        </section>
      {/if}
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
        <span class="grow"><b>{im.name}</b><small>{fmt(im.size)}{CHAIN.has(im.name) ? ' · ' + t('boot zinciri') : ''}</small></span><span class="sw"></span>
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

  {:else if tab === 'backup'}
    <section class="card">
      <div class="row" style="cursor:default"><DatabaseBackup size={26} /><span class="grow"><b>{t('Yedekle / Geri Yükle')}</b><small>{t('ADB Shell Root iznini verdiğinizden emin olun')}</small></span></div>
      <div class="frow"><div class="field"><input id="bo" bind:value={bkDir} /><label for="bo">{t('Yedek klasörü (bu telefonda)')}</label></div>
        <button class="btn tonal" aria-label="Klasör seç" onclick={async () => bkDir = await pick('dir', bkDir)}><FolderOpen size={22} /></button></div>
      <div class="group"><button class="btn" onclick={bkLoad} disabled={!adbInfo || !!busy}>{t('Uygulamalar')}</button><button class="btn" onclick={bkScan} disabled={!!busy}>{t('Yedekler')}</button></div>
      <div class="frow"><button class="btn tonal sm" style="flex:1" onclick={checkRoot} disabled={!adbInfo || !!busy}>{t('Root durumunu denetle')}</button>
        {#if rootState}<span class="chip ok">Root: {rootState}</span>{/if}</div>
      <div class="field"><input id="bq" bind:value={bkQ} /><label for="bq">{t('Paket adı ara')}</label></div>
      {#if !adbInfo}<p class="sub">{t('Yedeklemek için önce Cihaz sekmesinden ADB ile bağlan.')}</p>{/if}
      {#if (busy === 'Yedekleniyor' || busy === 'Geri yükleniyor') && owner === 'backup'}
        <div class="bar"><i style="width:{bkProg * 100}%"></i></div><p class="sub">{bkCur}</p>
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
    {#if bkItems.length}<button class="btn tonal wide" onclick={bkRestore} disabled={!adbInfo || !!busy || !countOn(bkItems, rPick, i => i.pkg)}>{t('Seçilenleri geri yükle')}</button>{/if}

  {:else}
    <div class="group">
      <button class="btn" class:sel={sub === 'general'} onclick={() => sub = 'general'}>{t('Genel')}</button>
      <button class="btn" class:sel={sub === 'log'} onclick={() => sub = 'log'}>{t('Günlük ({n})', { n: logs.length })}</button>
    </div>
    {#if sub === 'general'}
      <section class="card"><h2>{t('Dil')}</h2>
        <div class="group">{#each [['system','Sistem'],['tr','Türkçe'],['en','English']] as [v, l]}
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
      <h2>{t('{n} bölüm flaşlanacak', { n: steps.filter(s => !s.reboot).length })}</h2>
      <p class="sub">{t('Yanlış imaj cihazı bozabilir. Kabloyu çıkarma, uygulamayı kapatma ve ekranı açık tut.')}</p>
      <div class="row" class:on={ack} role="button" tabindex="0" onclick={() => ack = !ack} onkeydown={() => {}}>
        <span class="grow">{t('Yedeğim var, riski anlıyorum')}</span><span class="sw"></span></div>
      <div class="acts"><button class="btn text" onclick={() => confirmOpen = false}>{t('Vazgeç')}</button>
        <button class="btn danger" disabled={!ack} onclick={run}>{t('Başlat')}</button></div>
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
