/* Opt-in Muninn Browse: familiar thumbnail discovery from captured local feeds.
 * The original YouTube native webview stays available; this page never streams media. */
(function () {
  'use strict';
  const M=window.Muninn,L=M.L;
  const {h,setText,setHidden,setAttr}=M;
  const SOURCES={
    home:{title:'Home',hint:'Captured from the YouTube Home page'},
    subscriptions:{title:'Subscriptions',hint:'Captured from your YouTube subscriptions'},
    favourites:{title:'From your favourites',hint:'Uploads featured by your saved favourite-channel rules'},
    related:{title:'Related',hint:'Suggestions captured while viewing individual videos'},
  };
  const DEFAULT=['home','subscriptions','favourites','related'];
  function configured(value) {
    const input=Array.isArray(value)?value:DEFAULT,seen=new Set();
    const result=input.filter(id=>SOURCES[id]&&!seen.has(id)&&!!seen.add(id));
    return result.length?result:DEFAULT.slice();
  }
  function browsePage() {
    const state={order:configured(M.settings.browse_sections),filter:'all',data:{},loading:false,editing:false};
    const intro=h('p',{class:'browse-native-sub muted',text:'Your own Browse page, built from the YouTube feeds Muninn has captured. Nothing is streamed or requested from YouTube until you choose to refresh.'});
    const status=h('span',{class:'muted',role:'status'});
    const sections=h('div',{class:'browse-native-sections'});
    const editor=h('div',{class:'browse-native-editor',hidden:true});
    const filters=h('div',{class:'browse-native-filters',role:'group','aria-label':'Browse source'});
    const editButton=h('button',{type:'button',text:'Customize',onclick:()=>{
      state.editing=!state.editing;setHidden(editor,!state.editing);setAttr(editButton,'aria-expanded',String(state.editing));renderEditor();
    }});
    const original=h('button',{type:'button',text:'Open original YouTube',onclick:()=>{
      const tab=M.tabs.list.find(t=>t.id===M.lastBrowseTab&&t.face==='yt')||
                M.tabs.list.find(t=>t.face==='yt');
      if(tab)M.activateTab(tab.id);
      else M.newTab('https://www.youtube.com/');
    }});
    const reload=h('button',{type:'button',text:'Reload captured feeds',onclick:()=>load().catch(M.flashError)});
    const root=h('section',{class:'browse-native'},
      h('header',{class:'browse-native-header'},
        h('div',null,h('span',{class:'design-eyebrow',text:'Muninn · Browse'}),
          h('h1',{class:'h1',text:'Home'}),intro),
        h('div',{class:'browse-native-actions'},original,editButton,reload)),
      filters,editor,status,sections);

    function renderFilters() {
      const choices=[['all','All'],...state.order.map(id=>[id,SOURCES[id].title])];
      filters.replaceChildren(...choices.map(([id,title])=>
        h('button',{type:'button','aria-pressed':String(state.filter===id),text:title,
          onclick:()=>{state.filter=id;renderFilters();renderSections();}})));
    }
    function renderSections() {
      sections.replaceChildren();
      const active=state.filter==='all'?state.order:[state.filter];
      for(const id of active) {
        const data=state.data[id]||{},items=Array.isArray(data.items)?data.items:[];
        const last=data.captured_at ? L.relTime(data.captured_at) : null;
        const source=last ? 'Captured '+last : 'No local snapshot yet';
        const header=h('div',{class:'browse-native-section-head'},
          h('div',null,h('h2',{class:'h2',text:SOURCES[id].title}),
            h('span',{class:'muted',text:SOURCES[id].hint+' · '+source})));
        const panel=h('section',{class:'browse-native-section'},header,
          items.length ? h('div',{class:'browse-native-grid'},
            items.slice(0,30).map(item=>M.videoCard(item,video=>M.openVideo(video.video_id))))
          : h('p',{class:'empty-note',text:'Nothing captured for this source yet. Browse YouTube to fill it.'}));
        sections.append(panel);
      }
    }
    async function saveOrder(next) {
      const previous=state.order;
      state.order=configured(next);
      if(!state.order.includes(state.filter))state.filter='all';
      renderEditor();renderFilters();renderSections();
      try {
        const saved=await M.api('PUT','/api/settings',{browse_sections:state.order});
        M.settings={...M.settings,...saved};
        setText(status,'Browse layout saved.');
      } catch(error) {
        state.order=previous;renderEditor();renderFilters();renderSections();
        setText(status,'Could not save Browse layout: '+M.errText(error));
      }
    }
    function renderEditor() {
      if(!state.editing)return;
      const controls=state.order.map((id,i)=>h('div',{class:'browse-native-edit-row'},
        h('strong',{text:SOURCES[id].title}),
        h('button',{type:'button',text:'↑',disabled:i===0,'aria-label':'Move '+SOURCES[id].title+' up',
          onclick:()=>{const next=state.order.slice();[next[i-1],next[i]]=[next[i],next[i-1]];saveOrder(next);}}),
        h('button',{type:'button',text:'↓',disabled:i===state.order.length-1,
          'aria-label':'Move '+SOURCES[id].title+' down',
          onclick:()=>{const next=state.order.slice();[next[i+1],next[i]]=[next[i],next[i+1]];saveOrder(next);}}),
        h('button',{type:'button',text:'Hide',disabled:state.order.length===1,
          'aria-label':'Hide '+SOURCES[id].title,
          onclick:()=>saveOrder(state.order.filter(x=>x!==id))})));
      const missing=DEFAULT.filter(id=>!state.order.includes(id));
      const add=h('div',{class:'browse-native-add'},...missing.map(id=>h('button',{
        type:'button',text:'Show '+SOURCES[id].title,onclick:()=>saveOrder([...state.order,id])
      })));
      editor.replaceChildren(h('p',{class:'muted',text:'Choose and reorder the sources you see. This only changes your home layout; no captured data is deleted.'}),...controls,add);
    }
    async function load() {
      if(state.loading)return;
      state.loading=true;reload.disabled=true;setText(status,'Reading saved YouTube snapshots…');
      try {
        const urls=[
          M.api('GET','/api/feeds/latest?kind=home').catch(()=>null),
          M.api('GET','/api/feeds/latest?kind=subscriptions').catch(()=>null),
          M.api('GET','/api/featured').catch(()=>null),
          M.api('GET','/api/feeds/latest?kind=related').catch(()=>null)];
        const [home,subscriptions,favourites,related]=await Promise.all(urls);
        state.data={home,subscriptions,
          favourites:{items:favourites?.items||[],captured_at:favourites?.updated_at||null},
          related};
        setText(status,'Local captures · '+Object.values(state.data).filter(x=>x?.items?.length).length+' populated sources');
        renderSections();
      }catch(error){setText(status,'Could not load captures: '+M.errText(error));}
      finally{state.loading=false;reload.disabled=false;}
    }
    return {
      mount(el) {
        state.order=configured(M.settings.browse_sections);
        el.append(root);renderFilters();renderEditor();renderSections();load();
      },
      unmount() {root.remove();}
    };
  }
  M.pages.browse_custom=Object.assign({title:'Muninn Browse'},browsePage());
})();
