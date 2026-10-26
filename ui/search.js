/* Local-first universal search. Pure indexing helpers also run in Node tests. */
(function (root) {
  'use strict';
  const fold = value => String(value || '').normalize('NFKD').replace(/[\u0300-\u036f]/g, '').toLowerCase();
  function libraryIndex(entries) {
    const videos = new Map();
    for (const entry of entries) {
      const id = entry.video_id;
      if (!id) continue;
      const old = videos.get(id);
      const savedMedia = !!entry.protected || entry.state === 'protected';
      const savedLink = !!entry.saved_link || entry.state === 'link';
      const complete = ['complete', 'ready'].includes(entry.status) || (!entry.status && ['cached', 'ready', 'complete'].includes(entry.state));
      const next = old || {id, title:id, channel:'', savedMedia:false, savedLink:false, complete:false, downloading:false};
      if (entry.title && next.title === id) next.title = entry.title;
      if (entry.channel && !next.channel) next.channel = entry.channel;
      next.savedMedia ||= savedMedia;
      next.savedLink ||= savedLink;
      next.complete ||= complete;
      next.downloading ||= ['queued', 'downloading', 'partial'].includes(entry.state);
      next.saved = next.savedMedia || next.savedLink;
      next.kind = 'library';
      videos.set(id, next);
    }
    return [...videos.values()];
  }
  function savedChannels(known, configs, groups) {
    const explicit = new Set(configs.map(c => c.channel_id));
    for (const group of groups) for (const id of group.channel_ids || []) explicit.add(id);
    const channels = new Map();
    for (const c of [...known, ...configs]) {
      if (!c.channel_id) continue;
      if (explicit.has(c.channel_id) || c.source === 'configured' || (c.source === 'subscriptions' && !c.unsubscribed)) {
        const old = channels.get(c.channel_id);
        channels.set(c.channel_id, {kind:'channels', id:c.channel_id, title:old?.title || c.name || c.channel_id, channel:c.channel_id});
      }
    }
    for (const id of explicit) if (!channels.has(id)) channels.set(id, {kind:'channels',id,title:id,channel:id});
    return [...channels.values()];
  }
  function matches(rows, query) {
    const terms = fold(query).trim().split(/\s+/).filter(Boolean);
    return rows.map((row, order) => {
      const title = fold(row.title), text = title + ' ' + fold(row.channel) + ' ' + fold(row.url) + ' ' + fold(row.id);
      return {row, order, score:terms.every(t => text.includes(t)) ? (title === fold(query).trim() ? 3 : title.startsWith(fold(query).trim()) ? 2 : 1) : 0};
    }).filter(x => x.score).sort((a,b) => b.score-a.score || a.order-b.order).map(x => x.row);
  }
  function liveUrl(query, category) {
    const filters = {videos:'EgIQAQ==', live:'EgJAAQ==', channels:'EgIQAg=='};
    if (!filters[category] || !String(query).trim()) throw new Error('Choose a category and enter a search');
    return 'https://www.youtube.com/results?' + new URLSearchParams({search_query:String(query).trim(),sp:filters[category]});
  }
  const API = {libraryIndex, savedChannels, matches, liveUrl};
  if (typeof module === 'object' && module.exports) module.exports = API;
  root.MuninnSearch = API;
  if (!root.document || !root.Muninn) return;
  const M = root.Muninn, {h, icon} = M;
  let input, draw, timer, tabTimer, generation=0;
  const state = () => {
    const value=M.searchState ||= {};
    for(const [key,fallback] of Object.entries({query:'',scope:'all',saved:'all',category:'videos'})) value[key] ??= fallback;
    return value;
  };
  M.pages.search = {
    title:'Search',
    focus(query) {
      if (!input) return;
      if (query !== undefined) { input.value=query; state().query=query; draw?.(); }
      input.focus(); input.select();
    },
    mount(host) {
      const own=++generation, S=state();
      let data={library:[],channels:[],configs:[],groups:[]}, loading=true, failures=[], limit=20;
      const select = (label, options, key) => {
        const control=h('select', {'aria-label':label,onchange:()=>{S[key]=control.value;limit=20;draw();}}, ...options.map(([value,text])=>h('option',{value,text})));
        control.value=S[key] || options[0][0];
        return h('label',null,h('span',{text:label}),control);
      };
      input=h('input',{type:'search',id:'universal-query',value:S.query||'',placeholder:'Search tabs, videos and saved channels',autocomplete:'off',
        'aria-label':'Search Muninn',oninput:()=>{S.query=input.value;limit=20;clearTimeout(timer);timer=setTimeout(()=>draw(),150);}});
      const status=h('p',{class:'muted',role:'status','aria-live':'polite'});
      const results=h('div',{class:'search-results'});
      const live= h('button',{type:'submit',id:'search-youtube',class:'primary',text:'Search YouTube'});
      const onlineNote=h('p',{class:'muted'});
      const liveForm=h('form',{class:'search-live',onsubmit:event=>{
        event.preventDefault();
        if (!S.query?.trim() || M.offline || M.settings.offline_mode) return;
        M.newTab(liveUrl(S.query,S.category)).catch(M.flashError);
      }},h('h2',{class:'h3',text:'Search YouTube'}),h('p',{class:'muted',text:'Search online only when you choose this action.'}),
      select('YouTube category',[['videos','Videos'],['live','Live streams'],['channels','Channels']],'category'),live,onlineNote);
      const errors=h('div',{class:'search-errors'});
      const retry=h('button',{type:'button',text:'Refresh local results',onclick:()=>load()});
      const shell=h('div',{class:'search-page'},h('div',{class:'page-head'},h('div',null,h('h1',{class:'h1',text:'Search Muninn'}),h('p',{class:'muted',text:'Search this device first. Saved status and downloaded availability are separate.'}))),
        h('form',{class:'search-query',onsubmit:event=>{event.preventDefault();clearTimeout(timer);draw();}},input),
        h('div',{class:'search-filters'},select('Search in',[['all','All local'],['tabs','Open tabs'],['library','Library'],['channels','Saved channels']],'scope'),
          select('Library saved status',[['all','Saved and not saved'],['saved','Saved'],['unsaved','Not saved']],'saved'),retry),status,errors,results,liveForm);
      host.append(shell);
      const open = row => {
        const action=row.kind==='tabs'?()=>M.activateTab(row.id):row.kind==='library'?()=>M.openVideo(row.id):()=>M.openChannel(row.id);
        Promise.resolve().then(action).catch(M.flashError);
      };
      draw = () => {
        if (generation!==own) return;
        const videos=libraryIndex(data.library), byId=new Map(videos.map(v=>[v.id,v]));
        const tabs=M.tabs.list.map(tab=>({kind:'tabs',id:tab.id,title:M.L.tabTitle(tab),url:tab.url||'',channel:'',video:byId.get(tab.video_id || M.L.extractVideoId(tab.url))}));
        const library=videos.filter(v=>S.saved==='all'||!S.saved||(S.saved==='saved'?v.saved:!v.saved));
        const sections=[['tabs','Open tabs',tabs],['library','Library',library],['channels','Saved channels',savedChannels(data.channels,data.configs,data.groups)]];
        let total=0;
        const nodes=[];
        for (const [kind,label,rows] of sections) {
          if (S.scope && S.scope!=='all' && S.scope!==kind) continue;
          const found=matches(rows,S.query); total+=found.length;
          if (!found.length) continue;
          const section=h('section',{'aria-label':label},h('h2',{class:'h3',text:label+' · '+found.length}));
          for (const row of found.slice(0,limit)) {
            const video=row.kind==='library'?row:row.video;
            const availability=video ? (video.complete?'Downloaded':video.downloading?'Incomplete download':'No downloaded copy') : '';
            const savedDetail=video?.savedMedia?'Saved video':video?.savedLink?'Saved link':'';
            section.append(h('button',{type:'button',class:'search-result','data-search-kind':kind,'data-id':row.id,
              'data-saved':video?String(video.saved):null,onclick:()=>open(row)},icon(kind==='tabs'?'tabs':kind==='library'?'library':'channel'),
              h('span',{class:'search-result-text'},h('strong',{text:row.title}),h('span',{class:'muted',text:[row.channel||row.url,savedDetail,availability].filter(Boolean).join(' · ')})),
              video?h('span',{class:'badge '+(video.saved?'saved':'unsaved'),text:video.saved?'Saved':'Not saved'}):null));
          }
          if (found.length>limit) section.append(h('button',{type:'button',text:'Show more '+label.toLowerCase(),onclick:()=>{limit+=20;draw();}}));
          nodes.push(section);
        }
        results.replaceChildren(...nodes);
        status.textContent=loading?'Loading local results…':total+' local result'+(total===1?'':'s')+(failures.length?' · Some sources unavailable':'');
        if (!loading && !total) results.append(h('p',{class:'empty',text:'No local matches. Try another term or choose a YouTube search below.'}));
        const offline=M.offline||M.settings.offline_mode;
        live.disabled=!!offline||!S.query?.trim();
        onlineNote.textContent=offline?'YouTube search is unavailable while offline.':'';
      };
      async function load() {
        loading=true;retry.disabled=true;draw();
        const sources=[['library','/api/library'],['channels','/api/channels'],['configs','/api/subscriptions'],['groups','/api/channel-groups']];
        const loaded=await Promise.allSettled(sources.map(([,path])=>M.api('GET',path)));
        if (generation!==own) return;
        failures=[];
        loaded.forEach((result,i)=>{
          const [key]=sources[i];
          if(result.status==='fulfilled' && Array.isArray(result.value)) data[key]=result.value;
          else failures.push(key);
        });
        loading=false;retry.disabled=false;
        errors.replaceChildren(...(failures.length?[h('p',{role:'alert',text:'Could not refresh '+failures.map(k=>({configs:'saved channel settings',groups:'channel groups',channels:'channels',library:'library'}[k])).join(', ')+'. Refresh to retry; other results remain available.'})]:[]));
        draw();
      }
      load();
      // Open tab titles and saved/online controls may change while searching.
      let signature='';
      tabTimer=setInterval(()=>{
        const next=JSON.stringify([M.tabs.list.map(t=>[t.id,t.title,t.url,t.video_id]),M.offline,M.settings.offline_mode]);
        if(next!==signature) { signature=next;draw(); }
      },2000);
      input.focus();
    },
    unmount() { generation++;clearTimeout(timer);clearInterval(tabTimer);input=null;draw=null; },
  };
})(typeof window === 'object' ? window : globalThis);
