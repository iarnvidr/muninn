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
      const next = old || {id, title:id, channel:'',  savedMedia:false, savedLink:false, complete:false, downloading:false};
      if (entry.title && next.title === id) next.title = entry.title;
      if (entry.channel && !next.channel) next.channel = entry.channel;
      next.duration_seconds ||= entry.duration_seconds;
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
  function navigationDirection(event, editable) {
    if (editable || event.isComposing || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return null;
    return {ArrowLeft:'left',h:'left',ArrowDown:'down',j:'down',ArrowUp:'up',k:'up',ArrowRight:'right',l:'right'}[event.key] || null;
  }
  // Each band contains category columns; each column contains a header and rows.
  function moveCursor(bands, cursor, direction) {
    let {band,column,row}=cursor;
    const count=(b,c)=>bands[b]?.[c]||0;
    if (direction==='left'||direction==='right') {
      const step=direction==='left'?-1:1;
      for(let c=column+step;c>=0&&c<(bands[band]?.length||0);c+=step) {
        if(count(band,c)) return {band,column:c,row:Math.min(row,count(band,c)-1)};
      }
    } else {
      const step=direction==='up'?-1:1;
      if(row+step>=0&&row+step<count(band,column)) return {band,column,row:row+step};
      for(let b=band+step;b>=0&&b<bands.length;b+=step) {
        const c=count(b,column)?column:bands[b].findIndex(n=>n>0);
        if(c>=0) return {band:b,column:c,row:step>0?0:count(b,c)-1};
      }
      if(direction==='up') return null;
    }
    return cursor;
  }
  const API = {libraryIndex, savedChannels, matches, navigationDirection, moveCursor};
  if (typeof module === 'object' && module.exports) module.exports = API;
  root.MuninnSearch = API;
  if (!root.document || !root.Muninn) return;
  const M = root.Muninn, {h, icon} = M;
  let input, draw, timer, tabTimer, generation=0;
  const state = () => {
    const value=M.searchState ||= {};
    for(const [key,fallback] of Object.entries({query:'',scope:'all',saved:'all',onlineType:'videos'})) value[key] ??= fallback;
    if(!['videos','live','channels'].includes(value.onlineType)) value.onlineType='videos';
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
      let data={library:[],channels:[],configs:[],groups:[]}, loading=true, failures=[], limit=20, onlineRevision=0;
      const categories=[['videos','Videos'],['live','Live'],['channels','Channels']];
      S.online ||= {query:'',results:{},errors:{},pending:[]};
      if(S.online.pending?.length) {S.online.errors={...S.online.errors,...Object.fromEntries(S.online.pending.map(k=>[k,'Search interrupted. Press Enter to retry.']))};S.online.pending=[];}
      const filters = (label, options, key) => {
        const group=h('fieldset',{'data-filter':key},h('legend',{text:label}));
        for(const [value,text] of options) group.append(h('button',{type:'button',text,'data-value':value,'aria-pressed':String(S[key]===value),onclick:()=>{S[key]=value;limit=20;draw();}}));
        group.addEventListener('keydown',event=>{
          const direction=navigationDirection(event,false);
          if(!direction||!event.target.matches('button'))return;
          const buttons=[...group.querySelectorAll('button')],index=buttons.indexOf(event.target),step=['up','left'].includes(direction)?-1:1;
          event.preventDefault();buttons[(index+step+buttons.length)%buttons.length].focus();
        });
        return group;
      };
      const bandNodes=[h('div',{class:'search-columns','data-band':'0'}),h('div',{class:'search-columns','data-band':'1'})];
      const cursorOf=node=>{const cell=node?.closest('[data-search-cell]');return cell?{band:Number(cell.dataset.band),column:Number(cell.dataset.column),row:Number(cell.dataset.row)}:null;};
      const cells=()=>bandNodes.map(band=>[0,1,2].map(column=>[...band.querySelectorAll('[data-search-cell][data-column="'+column+'"]')]));
      const focusCursor=cursor=>{
        if(!cursor) {input.focus();return;}
        const node=cells()[cursor.band]?.[cursor.column]?.[cursor.row];
        if(node) {node.focus({preventScroll:true});node.scrollIntoView({block:'nearest',inline:'nearest'});}
      };
      const focusFirst=(bands=[1,0])=>{
        const matrix=cells();
        for(const band of bands) for(let column=0;column<3;column++) if(matrix[band][column].length>1) {focusCursor({band,column,row:1});return;}
      };
      input=h('input',{type:'search',id:'universal-query',value:S.query||'',placeholder:'Search Muninn or enter a YouTube address',autocomplete:'off',maxlength:200,
        'aria-label':'Search Muninn','aria-describedby':'search-help',oninput:()=>{S.query=input.value;limit=20;clearTimeout(timer);timer=setTimeout(()=>draw(),120);},
        onkeydown:event=>{if(event.key==='Enter'&&event.isComposing){event.preventDefault();return;}if(['ArrowDown','ArrowUp'].includes(event.key)&&!event.isComposing&&!event.metaKey&&!event.ctrlKey&&!event.altKey){event.preventDefault();clearTimeout(timer);draw();focusFirst(event.key==='ArrowUp'?[0]:[1]);}}});
      const status=h('p',{class:'muted',role:'status','aria-live':'polite'});
      const live=h('button',{type:'submit',id:'search-youtube',class:'primary',text:'Search online ↵'});
      const onlineNote=h('p',{class:'muted',role:'status','aria-live':'polite'});
      const errors=h('div',{class:'search-errors'});
      const retry=h('button',{type:'button',text:'Refresh local results',onclick:()=>load()});
      const rail=h('aside',{class:'search-filters','aria-label':'Search filters'},
        filters('Online type',categories,'onlineType'),
        filters('Local sources',[['all','All local'],['tabs','Open tabs'],['library','Library'],['channels','Saved channels']],'scope'),
        filters('Library saved status',[['all','Saved and not saved'],['saved','Saved'],['unsaved','Not saved']],'saved'),retry);
      const shell=h('div',{class:'search-page'},h('h1',{class:'h1',text:'Search Muninn'}),
        h('div',{class:'search-workspace'},rail,h('div',{class:'search-content'},
          h('section',{class:'search-band','aria-label':'Online results'},h('h2',{class:'h3',text:'Online · YouTube'}),onlineNote,bandNodes[0]),
          h('div',{class:'search-query-divider'},
            h('form',{class:'search-query',role:'search',onsubmit:event=>{event.preventDefault();if(!event.isComposing)submitOnline();}},input,live),
            h('p',{id:'search-help',class:'muted',text:'Enter searches online · ↑ online / ↓ local · arrows or h/j/k/l navigate results · Esc returns here'})),
          h('section',{class:'search-band','aria-label':'Local results'},h('h2',{class:'h3',text:'On this device'}),status,errors,bandNodes[1]))));
      host.append(shell);
      shell.addEventListener('keydown',event=>{
        const cursor=cursorOf(event.target);
        if(!cursor)return;
        if(event.key==='Escape') {event.preventDefault();input.focus();input.select();return;}
        const direction=navigationDirection(event,!!event.target.closest('input,textarea,[contenteditable="true"]'));
        if(!direction)return;
        event.preventDefault();
        if(cursor.band===0&&['left','right'].includes(direction)) {
          const index=categories.findIndex(([key])=>key===S.onlineType),step=direction==='left'?-1:1;
          S.onlineType=categories[(index+step+categories.length)%categories.length][0];
          draw();focusCursor({band:0,column:0,row:0});return;
        }
        focusCursor(moveCursor(cells().map(b=>b.map(c=>c.length)),cursor,direction));
      });
      const open = row => {
        const action=row.kind==='tabs'?()=>M.activateTab(row.id):(row.kind==='library'||row.kind==='video')?()=>M.openVideo(row.id):()=>M.openChannel(row.id);
        Promise.resolve().then(action).catch(M.flashError);
      };
      draw = () => {
        if (generation!==own) return;
        const focused=document.activeElement, focusKey=focused?.dataset.searchKey, previous=cursorOf(focused);
        const scrolls=bandNodes.map(band=>new Map([...band.querySelectorAll('.search-column')].map(column=>[column.dataset.column,column.querySelector('.search-column-body').scrollTop])));
        const videos=libraryIndex(data.library), byId=new Map(videos.map(v=>[v.id,v]));
        const tabs=M.tabs.list.map(tab=>{const videoId=tab.video_id||M.L.extractVideoId(tab.url);return {kind:'tabs',id:tab.id,title:M.L.tabTitle(tab),url:tab.url||'',channel:'',videoId,video:byId.get(videoId)};});
        const channelRows=savedChannels(data.channels,data.configs,data.groups), savedIds=new Set(channelRows.map(c=>c.id));
        const library=videos.filter(v=>S.saved==='all'||!S.saved||(S.saved==='saved'?v.saved:!v.saved));
        const column=(band,index,kind,label,rows,message)=>{
          const node=h('section',{class:'search-column','aria-label':label,'data-column':index});
          const props=row=>({'data-search-cell':'','data-band':band,'data-column':index,'data-row':row});
          const header=h('button',{type:'button',class:'search-type',...props(0),'data-search-key':band+'-'+kind+'-header',text:label+' · '+rows.length,onclick:()=>{
            if(band===0&&!rows.length) {submitOnline();return;}
            focusCursor({band,column:index,row:rows.length?1:0});
          }});
          const body=h('div',{class:'search-column-body'});
          for(const [index,row] of rows.slice(0,limit).entries()) {
            const video=row.kind==='library'?row:row.video;
            const availability=video?(video.complete?'Downloaded':video.downloading?'Incomplete download':'No downloaded copy'):'';
            const channel=row.kind==='channel'||row.kind==='channels';
            const videoId=row.kind==='tabs'?row.videoId:row.id;
            const hasImage=!channel&&/^[A-Za-z0-9_-]{11}$/.test(videoId||'');
            const saved=channel?savedIds.has(row.id):!!video?.saved;
            const image=hasImage?M.thumbImg('/thumb/'+videoId):null;
            if(image) {
              image.loading='lazy';
              let retries=0;
              image.addEventListener('error',()=>{if(retries++<3)setTimeout(()=>{if(image.isConnected)image.src=M.url('/thumb/'+videoId);},retries*1500);});
            }
            const duration=M.L.fmtDuration(row.duration_seconds||video?.duration_seconds);
            body.append(h('button',{type:'button',class:'search-result'+(channel?' search-channel':''),...props(index+1),'data-search-key':band+'-'+kind+'-'+row.id,
              'data-search-kind':kind,'data-id':row.id,'data-saved':String(saved),onclick:()=>open(row)},
              h('span',{class:'search-media'+(!hasImage?' search-media-fallback':''),'aria-hidden':'true'},
                image||h('span',{class:'search-avatar',text:channel?Array.from(row.title||'?')[0]:''},channel?null:icon(kind==='tabs'?'tabs':'library')),
                row.live?h('span',{class:'search-media-label live',text:'LIVE'}):duration?h('span',{class:'search-media-label',text:duration}):null),
              h('span',{class:'search-result-text'},h('strong',{text:row.title}),
                h('span',{class:'search-creator muted',text:row.channel||video?.channel||(channel?'YouTube channel':row.kind==='tabs'?'Open tab':'YouTube video')}),
                h('span',{class:'search-result-meta'},h('span',{class:'badge '+(saved?'saved':'unsaved'),text:saved?'Saved':'Not saved'}),
                  availability?h('span',{class:'muted',text:availability}):null))));
          }
          if(!rows.length)body.append(h('p',{class:'muted search-empty',text:message||'No matches'}));
          if(rows.length>limit)body.append(h('button',{type:'button',text:'Show more',...props(limit+1),'data-search-key':band+'-'+kind+'-more',onclick:()=>{const next=limit+1;limit+=20;draw();focusCursor({band,column:index,row:next});}}));
          node.append(header,body);return node;
        };
        const sameQuery=S.online.query===S.query.trim()&&!!S.query.trim();
        const onlineColumns=[];
        for(const [key,label] of categories) {
          if(S.onlineType!==key)continue;
          const found=sameQuery?(S.online.results[key]||[]).map(row=>({...row,video:row.kind==='video'?(byId.get(row.id)||{saved:false,complete:false}):null})):[];
          const pending=sameQuery&&S.online.pending.includes(key);
          onlineColumns.push(column(0,0,key,label,found,pending?'Searching…':sameQuery&&S.online.errors[key]?S.online.errors[key]:sameQuery&&Object.hasOwn(S.online.results,key)?'No online matches':'Press Enter to search '+label.toLowerCase()));
        }
        bandNodes[0].replaceChildren(...onlineColumns);
        bandNodes[0].dataset.columns=onlineColumns.length;
        const sections=[['tabs','Open tabs',tabs],['library','Library',library],['channels','Saved channels',channelRows]];
        let total=0;
        const nodes=[];
        for (const [index,[kind,label,rows]] of sections.entries()) {
          if (S.scope && S.scope!=='all' && S.scope!==kind) continue;
          const found=matches(rows,S.query); total+=found.length;
          nodes.push(column(1,index,kind,label,found,loading?'Loading…':'No local matches'));
        }
        bandNodes[1].replaceChildren(...nodes);
        bandNodes[1].dataset.columns=nodes.length;
        bandNodes.forEach((band,index)=>{for(const column of band.querySelectorAll('.search-column'))column.querySelector('.search-column-body').scrollTop=scrolls[index].get(column.dataset.column)||0;});
        status.textContent=loading?'Loading local results…':total+' local result'+(total===1?'':'s')+(failures.length?' · Some sources unavailable':'');
        const offline=M.offline||M.settings.offline_mode;
        live.disabled=!!offline||!S.query?.trim();
        onlineNote.textContent=offline?'Offline · local results remain available':sameQuery&&S.online.pending.includes(S.onlineType)?'Searching YouTube…':sameQuery&&Object.hasOwn(S.online.results,S.onlineType)?'Results for “'+S.online.query+'”':'Typing searches this device. Press Enter for the selected online type.';
        for(const group of rail.querySelectorAll('[data-filter]'))for(const button of group.querySelectorAll('button'))button.setAttribute('aria-pressed',String(S[group.dataset.filter]===button.dataset.value));
        if(focusKey) {
          const replacement=[...shell.querySelectorAll('[data-search-key]')].find(n=>n.dataset.searchKey===focusKey);
          if(replacement)replacement.focus({preventScroll:true});
          else if(previous)focusCursor(moveCursor(cells().map(b=>b.map(c=>c.length)),{...previous,row:0},'right'));
        }
      };
      async function submitOnline() {
        clearTimeout(timer);S.query=input.value;
        const query=S.query.trim();
        if(!query)return;
        if(/^(https?:\/\/|(?:www\.|m\.)?youtube\.com\/|youtu\.be\/)/i.test(query)){M.go(query).catch(M.flashError);return;}
        if(M.offline||M.settings.offline_mode){draw();return;}
        const revision=++onlineRevision;
        const category=S.onlineType;
        if(S.online.query!==query) S.online={query,results:{},errors:{},pending:[]};
        delete S.online.results[category];delete S.online.errors[category];
        S.online.pending=[category];draw();
        try {
          const result=await M.api('POST','/api/search',{query,category});
          if(generation!==own||revision!==onlineRevision)return;
          if(!Array.isArray(result?.items))throw Error('Invalid search response. Press Enter to retry.');
          S.online.results[category]=result.items;
        } catch(error) {
          if(generation!==own||revision!==onlineRevision)return;
          S.online.errors[category]=M.errText(error);
        } finally {
          if(generation===own&&revision===onlineRevision){
            S.online.pending=S.online.pending.filter(k=>k!==category);draw();
            const cursor=cursorOf(document.activeElement);
            if(S.query.trim()===query&&S.onlineType===category&&(document.activeElement===input||cursor?.band===0&&cursor.row===0))focusFirst([0]);
          }
        }
      }
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
