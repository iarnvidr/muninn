/* Focused, provenance-backed feed explorer.
 * Default shows channel groups in one capture, not every video and edge at once.
 * Drill into a channel or video to see only directly observed relationships.
 * No external graph service, inferred recommendations, or HTML from metadata. */
(function(root) {
  'use strict';
  const NS='http://www.w3.org/2000/svg';
  const MAX_SNAPSHOTS=20, MAX_ITEMS_PER_SNAPSHOT=160, MAX_VIDEOS=800, MAX_CHANNELS=240;
  const PAGE_SIZE=8;
  const KINDS={home:'Home',subscriptions:'Subscriptions',search:'Search',related:'Related',
    channel:'Channel',other:'Other'};
  const shorten=(s,n)=>{s=String(s||'');return s.length>n?s.slice(0,n-1)+'…':s;};
  const validVideo=s=>/^[A-Za-z0-9_-]{11}$/.test(s);
  const validChannel=s=>/^UC[A-Za-z0-9_-]{22}$/.test(s);
  const byId=(a,b)=>a.id.localeCompare(b.id);
  const nameOf=s=>String(s||'').trim();
  const addTo=(map,key,value)=>{if(!map.has(key))map.set(key,new Set());map.get(key).add(value);};

  function build(captures) {
    const input=Array.isArray(captures)?captures:[];
    const nodes=new Map(),edges=new Map(),videos=new Set(),channels=new Set();
    let truncated=input.length>MAX_SNAPSHOTS,observed=0,validSnapshots=0;
    const add=(id,kind,label,extra)=>{
      if(!nodes.has(id))nodes.set(id,{id,kind,label:shorten(label,220),...(extra||{})});
    };
    const edge=(a,b,relation)=>{
      if(nodes.has(a)&&nodes.has(b))edges.set(a+'|'+b+'|'+relation,
        {id:a+'|'+b+'|'+relation,source:a,target:b,relation});
    };
    for(const s of input.slice(0,MAX_SNAPSHOTS)){
      const numeric=Number(s?.id);
      if(!Number.isSafeInteger(numeric)||numeric<=0)continue;
      validSnapshots++;
      const kind=Object.prototype.hasOwnProperty.call(KINDS,s.kind)?s.kind:'other';
      const source='source:'+kind,snapshot='snapshot:'+numeric;
      const query=nameOf(s.query);
      const label=KINDS[kind]+(query?' · '+shorten(query,52):'');
      add(source,'source',KINDS[kind],{kindName:kind});
      add(snapshot,'snapshot',label,{snapshot_id:numeric,source_url:nameOf(s.source_url),
        captured_at:s.captured_at,count:s.count,query,kindName:kind});
      edge(source,snapshot,'capture');
      const items=Array.isArray(s.items)?s.items:[];
      if(items.length>MAX_ITEMS_PER_SNAPSHOT)truncated=true;
      for(const item of items.slice(0,MAX_ITEMS_PER_SNAPSHOT)){
        const videoId=nameOf(item?.video_id);
        if(!validVideo(videoId))continue;
        const video='video:'+videoId;
        if(!videos.has(videoId)&&videos.size>=MAX_VIDEOS){truncated=true;continue;}
        videos.add(videoId);observed++;
        add(video,'video',nameOf(item.title)||videoId,{video_id:videoId,
          channel:nameOf(item.channel)});
        edge(snapshot,video,'contains');
        const rawId=nameOf(item.channel_id),channelName=nameOf(item.channel);
        // Without a verified channel ID, keep name-only clusters scoped to their
        // capture: an identical display name in another capture is not identity proof.
        const channelId=validChannel(rawId)
          ? 'channel:'+rawId
          : 'channel-local:'+numeric+':'+(channelName.toLowerCase()||'unknown');
        if(!channels.has(channelId)&&channels.size>=MAX_CHANNELS){truncated=true;continue;}
        channels.add(channelId);
        const known=validChannel(rawId);
        add(channelId,'channel',channelName||(known?rawId:'Channel not recorded'),{
          channel_id:known?rawId:null,verified_id:known,
          explanation:known?'Channel ID recorded by YouTube':'Channel ID not recorded; grouped within this capture only'});
        edge(channelId,video,'published');
      }
    }
    const values=[...nodes.values()],connections=[...edges.values()],degree=new Map();
    for(const n of values)degree.set(n.id,0);
    for(const e of connections){
      degree.set(e.source,degree.get(e.source)+1);degree.set(e.target,degree.get(e.target)+1);
    }
    for(const n of values)n.degree=degree.get(n.id);
    return {nodes:values,edges:connections,snapshots:validSnapshots,
      observed,truncated,labels:KINDS};
  }

  // Retain the original pure layout helper's public contract for existing callers.
  function positions(graph,width=1000,height=640) {
    const out=new Map(),order=['source','snapshot','channel','video'];
    for(const [column,kind] of order.entries()){
      const group=(graph.nodes||[]).filter(n=>n.kind===kind).sort(byId);
      group.forEach((n,i)=>out.set(n.id,{x:60+column*260,
        y:32+(i+.5)*(Math.max(80,height-64)/Math.max(group.length,1))}));
    }
    return out;
  }

  function relationships(graph) {
    const nodes=new Map((graph.nodes||[]).map(n=>[n.id,n]));
    const captureVideos=new Map(),videoCaptures=new Map(),channelVideos=new Map(),videoChannels=new Map();
    for(const edge of graph.edges||[]){
      if(edge.relation==='contains'){
        addTo(captureVideos,edge.source,edge.target);
        addTo(videoCaptures,edge.target,edge.source);
      }
      if(edge.relation==='published'){
        addTo(channelVideos,edge.source,edge.target);
        addTo(videoChannels,edge.target,edge.source);
      }
    }
    return {nodes,captureVideos,videoCaptures,channelVideos,videoChannels};
  }

  // Exported pure model: paging and filtering happen before layout, and all
  // aggregation is traceable back to explicit snapshot→video←channel edges.
  function overview(graph,{snapshotId=null,scope='selected',query='',offset=0,limit=PAGE_SIZE}={}) {
    const r=relationships(graph);
    const captures=(graph.nodes||[]).filter(n=>n.kind==='snapshot');
    const selected=r.nodes.get(snapshotId)||captures[0]||null;
    const active=scope==='all'?captures:(selected?[selected]:[]);
    const videoIds=new Set();
    for(const s of active)for(const id of r.captureVideos.get(s.id)||[])videoIds.add(id);
    const groups=new Map();
    for(const id of videoIds) {
      const linked=r.videoChannels.get(id)||new Set();
      for(const chId of linked) {
        const node=r.nodes.get(chId);if(!node)continue;
        if(!groups.has(chId))groups.set(chId,{id:chId,node,videos:new Set(),captures:new Set()});
        const group=groups.get(chId);group.videos.add(id);
        for(const snap of r.videoCaptures.get(id)||[])group.captures.add(snap);
      }
    }
    const q=nameOf(query).toLocaleLowerCase();
    const all=[...groups.values()].map(g=>({
      ...g,count:g.videos.size,sharedCaptures:g.captures.size,
      matches:!q||g.node.label.toLocaleLowerCase().includes(q)||
        [...g.videos].some(v=>r.nodes.get(v)?.label.toLocaleLowerCase().includes(q))
    })).filter(g=>g.matches)
      .sort((a,b)=>b.count-a.count||b.sharedCaptures-a.sharedCaptures||a.node.label.localeCompare(b.node.label));
    const size=Math.max(1,Math.min(12,Number(limit)||PAGE_SIZE));
    const start=Math.min(Math.max(0,Math.floor(Number(offset)||0)),Math.max(0,all.length-1));
    return {selected,active,videoCount:videoIds.size,totalGroups:all.length,groups:all,
      visible:all.slice(start,start+size),start,end:Math.min(start+size,all.length),
      more:start+size<all.length,previous:start>0,truncated:!!graph.truncated};
  }

  function mount(container,callbacks={}) {
    const doc=container.ownerDocument;
    const html=(tag,klass,text)=>{
      const el=doc.createElement(tag);if(klass)el.className=klass;
      if(text!==undefined)el.textContent=String(text);return el;
    };
    const svgEl=(tag,attrs)=>{
      const el=doc.createElementNS(NS,tag);
      for(const [k,v] of Object.entries(attrs||{}))el.setAttribute(k,String(v));
      return el;
    };
    const button=(label,fn,klass)=>{
      const b=html('button',klass||'',label);b.type='button';b.addEventListener('click',fn);return b;
    };
    const toolbar=html('div','feed-explorer-toolbar');
    const scopeLabel=html('label','feed-explorer-field','Scope');
    const scopeSelect=html('select');
    scopeSelect.setAttribute('aria-label','Connections scope');
    for(const [value,label] of [['selected','Selected capture'],['all','All loaded captures']]){
      const option=html('option',null,label);option.value=value;scopeSelect.append(option);
    }
    scopeLabel.append(scopeSelect);
    const search=html('input');search.type='search';search.placeholder='Find a channel or video';
    search.setAttribute('aria-label','Filter feed connections');
    const back=button('All channels',()=>{focus=null;offset=0;draw();},'feed-explorer-back');
    const previous=button('Previous',()=>{offset=Math.max(0,offset-PAGE_SIZE);draw();});
    const next=button('Next',()=>{offset+=PAGE_SIZE;draw();});
    const zoomOut=button('−',()=>zoomTo(zoom/1.2));zoomOut.setAttribute('aria-label','Zoom out');
    const zoomIn=button('+',()=>zoomTo(zoom*1.2));zoomIn.setAttribute('aria-label','Zoom in');
    const fit=button('Fit',()=>{zoom=1;panX=0;panY=0;setView();});
    const pageStatus=html('span','feed-explorer-page');
    toolbar.append(scopeLabel,search,back,previous,pageStatus,next,zoomOut,zoomIn,fit);
    const layout=html('div','feed-explorer-layout');
    const stage=html('div','feed-explorer-stage');
    const heading=html('div','feed-explorer-heading');
    const canvas=html('div','feed-explorer-canvas');
    const svg=svgEl('svg',{viewBox:'0 0 1000 680',role:'group',
      'aria-label':'Observed relationships between captures, channels and videos'});
    svg.setAttribute('tabindex','0');
    canvas.append(svg);stage.append(heading,canvas);
    const inspector=html('aside','feed-explorer-inspector');
    inspector.setAttribute('aria-label','Connection details');
    const legend=html('p','feed-explorer-legend',
      'Only observed data. Channel groups summarize captured videos; select a channel or video to expand its actual links.');
    layout.append(stage,inspector);
    container.replaceChildren(toolbar,layout,legend);

    let graph={nodes:[],edges:[],snapshots:0},rel=relationships(graph);
    let snapshotId=null,focus=null,scope='selected',offset=0,query='',zoom=1,panX=0,panY=0;
    let viewHeight=680,viewWidth=1000;
    const title=(parent,text,tag='h3')=>{const x=html(tag,'',text);parent.append(x);return x;};
    const paragraph=(parent,text,cls='')=>{const el=html('p',cls,text);parent.append(el);return el;};
    const line=(x1,y1,x2,y2)=>{
      const m=Math.round((x1+x2)/2),p=svgEl('path',{
        d:'M'+x1+' '+y1+' C'+m+' '+y1+' '+m+' '+y2+' '+x2+' '+y2,
        class:'feed-explorer-edge'});svg.append(p);
    };
    const card=(node,x,y,width,height,label,sub,onClick,onOpen,kind='item')=>{
      const group=svgEl('g',{class:'feed-explorer-node '+kind,role:'button',tabindex:'0',
        'aria-label':label+(sub?' · '+sub:'')});
      group.setAttribute('transform','translate('+x+','+y+')');
      const rect=svgEl('rect',{x:0,y:0,width,height,rx:11});
      const text=svgEl('text',{x:16,y:height/2-3,class:'feed-explorer-label'});
      text.textContent=shorten(label,width>340?46:28);
      const small=svgEl('text',{x:16,y:height/2+17,class:'feed-explorer-sub'});
      small.textContent=shorten(sub,width>340?61:34);
      const tooltip=svgEl('title');tooltip.textContent=label+(sub?' · '+sub:'');
      group.append(rect,text,small,tooltip);
      group.addEventListener('click',onClick);
      if(onOpen)group.addEventListener('dblclick',onOpen);
      group.addEventListener('keydown',e=>{
        if(e.key==='Enter'||e.key===' '){e.preventDefault();onClick();}
      });
      svg.append(group);return group;
    };
    function setView(){
      const w=viewWidth/zoom,h=viewHeight/zoom;
      const x=(viewWidth-w)/2+panX,y=(viewHeight-h)/2+panY;
      svg.setAttribute('viewBox',[x,y,w,h].join(' '));
    }
    function zoomTo(value){zoom=Math.max(.8,Math.min(2.5,value));setView();}
    function capturedDate(ts){
      const numeric=Number(ts),date=Number.isFinite(numeric)&&numeric>0
        ? new Date(numeric>1e12?numeric:numeric*1000):new Date(ts);
      return Number.isNaN(date.getTime())?'Unknown date':date.toLocaleDateString();
    }
    function sourceSummary(node){
      return node.source_url?'Recorded source: '+node.source_url:'Source URL was not recorded for this capture.';
    }
    function openCapture(s){
      if(!s)return;
      if(snapshotId!==s.id){snapshotId=s.id;callbacks.onSelect?.(s);}
      focus=null;offset=0;scope='selected';scopeSelect.value=scope;draw();
    }
    function inspectorButton(label,fn){
      inspector.append(button(label,fn,'feed-explorer-action'));
    }
    function detailList(name,items,callback,max=18){
      title(inspector,name,'h4');
      const list=html('ul','feed-explorer-inspector-list');
      for(const row of items.slice(0,max)){
        const li=html('li');const b=button(row.label,()=>callback(row));
        b.title=row.label;li.append(b);list.append(li);
      }
      inspector.append(list);
      if(items.length>max)paragraph(inspector,(items.length-max)+' additional items not listed here. Use graph pagination.');
    }
    const getVideo=id=>rel.nodes.get(id);
    const captureIdsOf=id=>[...(rel.videoCaptures.get(id)||[])];
    function inspectOverview(model) {
      title(inspector,scope==='all'?'Loaded feed captures':'Selected capture');
      if(model.selected){
        paragraph(inspector,model.selected.label,'feed-explorer-strong');
        paragraph(inspector,sourceSummary(model.selected));
        paragraph(inspector,'Captured '+capturedDate(model.selected.captured_at));
      }
      paragraph(inspector,model.active.length+' capture'+(model.active.length===1?'':'s')+
        ' · '+model.videoCount+' unique videos · '+model.totalGroups+' channel groups');
      if(graph.truncated)paragraph(inspector,'Some older captures or items were excluded to keep the explorer responsive.','feed-explorer-warning');
      if(scope==='selected'&&model.selected)
        inspectorButton('View snapshot cards',()=>callbacks.onOpen?.(model.selected));
      paragraph(inspector,'Choose a channel group to inspect the videos and the other captures containing them.');
      if(!model.totalGroups)paragraph(inspector,query?'No channel or video matches this filter.':'No recorded video/channel connections for this capture.');
    }
    function inspectChannel(ch,videos) {
      title(inspector,ch.label);
      paragraph(inspector,ch.verified_id?'Verified channel ID in recorded metadata.':'Channel name only; its identity across captures is not verified.');
      paragraph(inspector,videos.length+' recorded video'+(videos.length===1?'':'s')+
        ' across '+new Set(videos.flatMap(v=>captureIdsOf(v.id))).size+' capture(s)');
      if(ch.channel_id)inspectorButton('Open channel',()=>callbacks.onOpen?.(ch));
      detailList('Videos',videos.map(v=>({id:v.id,label:v.label})),r=>{focus={kind:'video',id:r.id};offset=0;draw();},24);
    }
    function inspectVideo(v) {
      title(inspector,v.label);
      paragraph(inspector,'Observed video · '+v.video_id);
      inspectorButton('Play in Muninn',()=>callbacks.onOpen?.(v));
      const channels=[...(rel.videoChannels.get(v.id)||[])].map(id=>rel.nodes.get(id)).filter(Boolean);
      detailList('Recorded channels',channels.map(ch=>({id:ch.id,label:ch.label})),row=>{
        focus={kind:'channel',id:row.id};offset=0;draw();
      });
      const snapshots=captureIdsOf(v.id).map(id=>rel.nodes.get(id)).filter(Boolean);
      detailList('Captured in',snapshots.map(s=>({id:s.id,label:s.label+' · '+capturedDate(s.captured_at)})),row=>{
        const snapshot=rel.nodes.get(row.id);openCapture(snapshot);
      });
    }
    function rowsForChannel(ch){
      const q=query.toLocaleLowerCase();
      return [...(rel.channelVideos.get(ch.id)||[])].map(id=>getVideo(id)).filter(Boolean)
        .filter(v=>!q||ch.label.toLocaleLowerCase().includes(q)||v.label.toLocaleLowerCase().includes(q))
        .sort((a,b)=>captureIdsOf(b.id).length-captureIdsOf(a.id).length||a.label.localeCompare(b.label));
    }
    function buildFrame(rootNode,rootLabel,rootSubtitle,rows,actions,kind) {
      svg.replaceChildren();
      const h=Math.max(630,70+rows.length*73);
      viewHeight=h;viewWidth=1000;svg.style.height=Math.min(h,710)+'px';
      // Source/snapshot chain is described in the inspector, not drawn twice.
      const rootY=h/2-43;
      for(let i=0;i<rows.length;i++)line(275,rootY+43,398,78+i*73);
      card(rootNode,24,rootY,251,86,rootLabel,rootSubtitle,actions?.root||(()=>{}),actions?.openRoot,'root');
      rows.forEach((row,i)=>{
        card(row.node,398,47+i*73,568,62,row.label,row.sub,row.click,row.open,kind);
      });
      setView();
    }
    function draw() {
      rel=relationships(graph);inspector.replaceChildren();
      const captures=[...rel.nodes.values()].filter(n=>n.kind==='snapshot');
      if(!rel.nodes.get(snapshotId))snapshotId=captures[0]?.id||null;
      const selected=rel.nodes.get(snapshotId);
      let rows=[],rootNode=null,rootLabel='',rootSub='',kind='channel';
      let actions=null,caption='';
      if(focus?.kind==='channel'&&rel.nodes.get(focus.id)){
        const ch=rel.nodes.get(focus.id),videos=rowsForChannel(ch);
        const shown=videos.slice(offset,offset+PAGE_SIZE);
        rootNode=ch;rootLabel=ch.label;rootSub=videos.length+' videos';
        rows=shown.map(v=>({node:v,label:v.label,sub:'Seen in '+captureIdsOf(v.id).length+' capture(s)',
          click:()=>{focus={kind:'video',id:v.id};offset=0;draw();},
          open:()=>callbacks.onOpen?.(v)}));
        actions={root:()=>{focus=null;offset=0;draw();},openRoot:ch.channel_id?()=>callbacks.onOpen?.(ch):null};
        caption='Channel → recorded videos';inspectChannel(ch,videos);
        pageStatus.textContent=(offset+1)+'–'+Math.min(offset+PAGE_SIZE,videos.length)+' of '+videos.length;
        previous.disabled=offset===0;next.disabled=offset+PAGE_SIZE>=videos.length;
      }else if(focus?.kind==='video'&&rel.nodes.get(focus.id)){
        const v=rel.nodes.get(focus.id);rootNode=v;rootLabel=v.label;rootSub='Video · '+v.video_id;
        const linked=[
          ...[...(rel.videoChannels.get(v.id)||[])].map(id=>rel.nodes.get(id)).filter(Boolean),
          ...captureIdsOf(v.id).map(id=>rel.nodes.get(id)).filter(Boolean)
        ];const shown=linked.slice(offset,offset+PAGE_SIZE);
        rows=shown.map(n=>({node:n,label:n.label,
          sub:n.kind==='snapshot'?'Captured '+capturedDate(n.captured_at):'Recorded channel',
          click:()=>{if(n.kind==='snapshot')openCapture(n);else{focus={kind:'channel',id:n.id};offset=0;draw();}},
          open:()=>callbacks.onOpen?.(n)}));
        actions={root:()=>callbacks.onOpen?.(v),openRoot:()=>callbacks.onOpen?.(v)};
        caption='Video → channel and captured snapshots';inspectVideo(v);kind='context';
        pageStatus.textContent=(offset+1)+'–'+Math.min(offset+PAGE_SIZE,linked.length)+' of '+linked.length;
        previous.disabled=offset===0;next.disabled=offset+PAGE_SIZE>=linked.length;
      }else{
        focus=null;
        const m=overview(graph,{snapshotId,scope,query,offset});
        if(m.start!==offset)offset=m.start;
        rootNode=selected||{id:'empty',kind:'snapshot',label:'No captures'};
        rootLabel=scope==='all'?'Loaded captures':(selected?.label||'No captures');
        rootSub=scope==='all'?m.active.length+' captures · '+m.videoCount+' videos'
          :m.videoCount+' videos · '+m.totalGroups+' channels';
        rows=m.visible.map(g=>({node:g.node,label:g.node.label,
          sub:g.count+' video'+(g.count===1?'':'s')+' · '+g.sharedCaptures+' capture(s)',
          click:()=>{focus={kind:'channel',id:g.id};offset=0;draw();},
          open:g.node.channel_id?()=>callbacks.onOpen?.(g.node):null}));
        actions={root:()=>{if(selected)callbacks.onOpen?.(selected);}};
        caption=scope==='all'?'Channels grouped across loaded captures':'Channels in '+(selected?.label||'capture');
        inspectOverview(m);
        pageStatus.textContent=m.totalGroups?m.start+1+'–'+m.end+' of '+m.totalGroups:'0 channels';
        previous.disabled=!m.previous;next.disabled=!m.more;
      }
      back.hidden=!focus;
      heading.textContent=caption;
      scopeSelect.disabled=!captures.length;
      search.disabled=!captures.length;
      if(!rows.length){
        svg.replaceChildren();
        const empty=svgEl('text',{x:500,y:270,'text-anchor':'middle',class:'feed-explorer-empty'});
        empty.textContent=query?'No matches — try another search':'No connections to display in this view';
        svg.append(empty);viewHeight=630;svg.style.height='530px';setView();
      }else buildFrame(rootNode,rootLabel,rootSub,rows,actions,kind);
    }
    scopeSelect.addEventListener('change',()=>{scope=scopeSelect.value;offset=0;focus=null;draw();});
    search.addEventListener('input',()=>{query=search.value.trim();offset=0;draw();});
    let drag=null;
    svg.addEventListener('pointerdown',e=>{
      if(e.target.closest?.('.feed-explorer-node'))return;
      drag={x:e.clientX,y:e.clientY,px:panX,py:panY};
      svg.setPointerCapture?.(e.pointerId);
    });
    svg.addEventListener('pointermove',e=>{
      if(!drag)return;
      const r=svg.getBoundingClientRect();
      panX=drag.px-(e.clientX-drag.x)*(viewWidth/zoom)/(r.width||1000);
      panY=drag.py-(e.clientY-drag.y)*(viewHeight/zoom)/(r.height||680);
      setView();
    });
    svg.addEventListener('pointerup',()=>{drag=null;});
    svg.addEventListener('pointercancel',()=>{drag=null;});
    svg.addEventListener('wheel',e=>{e.preventDefault();zoomTo(zoom*(e.deltaY>0?.89:1.12));},{passive:false});
    return {
      render(data,id){
        graph=data||{nodes:[],edges:[],snapshots:0};const selected=rel.nodes.get(id);
        if(id&&graph.nodes?.some(n=>n.id===id&&n.kind==='snapshot'))snapshotId=id;
        else if(!graph.nodes?.some(n=>n.id===snapshotId))snapshotId=graph.nodes?.find(n=>n.kind==='snapshot')?.id||null;
        focus=null;offset=0;draw();
      },
      select(id){
        if(!id||id===snapshotId)return;
        if(graph.nodes?.some(n=>n.id===id&&n.kind==='snapshot')){
          snapshotId=id;focus=null;offset=0;draw();
        }
      },
      destroy(){container.replaceChildren();}
    };
  }
  const api={build,positions,overview,relationships,mount};
  if(typeof module!=='undefined'&&module.exports)module.exports=api;
  if(root)root.MuninnFeedGraph=api;
})(typeof window!=='undefined'?window:null);
