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


  // Chat Hub-style network presentation. Every aggregate edge is justified by
  // snapshot -> video <- channel links; no inferred channel relationships.
  function prepareNetwork(graph,{snapshotId=null,scope='selected',query='',focusId=null,maxChannels=42}={}) {
    const rel=relationships(graph);
    const snapshots=[...rel.nodes.values()].filter(n=>n.kind==='snapshot');
    const selected=rel.nodes.get(snapshotId)||snapshots[0]||null;
    const captures=scope==='all'?snapshots.slice(0,16):(selected?[selected]:[]);
    const inScope=new Set();
    for(const snap of captures)for(const id of rel.captureVideos.get(snap.id)||[])inScope.add(id);
    const channelGroups=new Map();
    for(const vid of inScope)for(const chId of rel.videoChannels.get(vid)||[]) {
      const node=rel.nodes.get(chId);if(!node)continue;
      if(!channelGroups.has(chId))channelGroups.set(chId,{node,videoIds:new Set(),captureIds:new Set()});
      const g=channelGroups.get(chId);g.videoIds.add(vid);
      for(const captureId of rel.videoCaptures.get(vid)||[])if(captures.some(s=>s.id===captureId))g.captureIds.add(captureId);
    }
    const q=String(query||'').toLocaleLowerCase().trim();
    const ranked=[...channelGroups.values()].filter(g=>!q||g.node.label.toLocaleLowerCase().includes(q)||
      [...g.videoIds].some(id=>rel.nodes.get(id)?.label.toLocaleLowerCase().includes(q)))
      .sort((a,b)=>b.videoIds.size-a.videoIds.size||b.captureIds.size-a.captureIds.size||a.node.label.localeCompare(b.node.label));
    const selectedGroup=ranked.find(g=>g.node.id===focusId)||
      channelGroups.get(focusId);
    let included=ranked.slice(0,Math.max(8,Math.min(100,maxChannels)));
    if(selectedGroup&&!included.includes(selectedGroup))included=[...included.slice(0,Math.max(0,included.length-1)),selectedGroup];
    const nodeMap=new Map(),edgeMap=new Map();
    const add=(node,extra={})=>{if(!nodeMap.has(node.id))nodeMap.set(node.id,{...node,...extra});};
    const edge=(source,target,basis,count,reason)=>{
      if(!nodeMap.has(source)||!nodeMap.has(target))return;
      const id=source+'|'+target+'|'+basis;
      edgeMap.set(id,{id,source,target,basis,count,reason});
    };
    for(const snap of captures)add(snap,{degree:0,kind:'snapshot'});
    for(const group of included)add(group.node,{degree:0,count:group.videoIds.size,kind:'channel'});
    for(const group of included)for(const capture of group.captureIds){
      let count=0;for(const vid of group.videoIds)if(rel.videoCaptures.get(vid)?.has(capture))count++;
      if(count)edge(capture,group.node.id,'captured_in',count,count+' recorded video'+(count===1?'':'s')+' from this channel in this capture');
    }
    // Expand only an actual one-hop neighborhood on selection, never 100
    // overlapping labels. The inspector still exposes every stored video.
    let expansionVideos=[];
    if(focusId?.startsWith('channel:')||focusId?.startsWith('channel-local:')) {
      const ids=[...(channelGroups.get(focusId)?.videoIds||[])].sort((a,b)=>
        (rel.videoCaptures.get(b)?.size||0)-(rel.videoCaptures.get(a)?.size||0)||
        (rel.nodes.get(a)?.label||'').localeCompare(rel.nodes.get(b)?.label||''));
      expansionVideos=ids.slice(0,24);
    }else if(focusId?.startsWith('video:')&&inScope.has(focusId)){
      expansionVideos=[focusId];
      for(const id of rel.videoChannels.get(focusId)||[]) {
        const ch=rel.nodes.get(id);if(ch)add(ch,{count:channelGroups.get(id)?.videoIds.size||1});
      }
    }
    for(const vid of expansionVideos){
      const v=rel.nodes.get(vid);if(!v)continue;
      add(v,{degree:0,kind:'video'});
      for(const ch of rel.videoChannels.get(vid)||[]) {
        if(nodeMap.has(ch))edge(ch,vid,'published',1,'Channel published this recorded video');
      }
      for(const snap of rel.videoCaptures.get(vid)||[]) {
        if(nodeMap.has(snap))edge(snap,vid,'contains',1,'Video appeared in this captured feed');
      }
    }
    // Each visible cluster comes from observed capture-channel relationships.
    const adjacency=new Map([...nodeMap.keys()].map(id=>[id,new Set()]));
    for(const e of edgeMap.values()){adjacency.get(e.source).add(e.target);adjacency.get(e.target).add(e.source);}
    const visited=new Set();let cluster=0;
    for(const seed of nodeMap.keys()){
      if(visited.has(seed))continue;
      const queue=[seed],members=[];for(let i=0;i<queue.length;i++){
        const id=queue[i];if(visited.has(id))continue;visited.add(id);members.push(id);
        queue.push(...adjacency.get(id));
      }
      const sorted=members.sort((a,b)=>adjacency.get(b).size-adjacency.get(a).size||a.localeCompare(b));
      const hub=sorted[0];const landmarks=new Set([hub]),reserved=new Set();
      const reserve=id=>{reserved.add(id);for(const n of adjacency.get(id))reserved.add(n);};
      reserve(hub);
      for(const id of sorted){if(landmarks.size>=8)break;
        if(!reserved.has(id)&&adjacency.get(id).size>1){landmarks.add(id);reserve(id);}
      }
      for(const id of members){const n=nodeMap.get(id);n.cluster=cluster;n.degree=adjacency.get(id).size;
        n.hub=id===hub;n.landmark=landmarks.has(id)||(n.kind==='snapshot');
      }cluster++;
    }
    // A single capture connects to every channel, so the generic hub-only
    // landmark rule would label only that capture. Keep a handful of the
    // highest-evidence channels eligible for collision-checked labels too.
    for(const g of included.slice(0,10)){
      const visible=nodeMap.get(g.node.id);if(visible)visible.landmark=true;
    }
    const nodes=[...nodeMap.values()],edges=[...edgeMap.values()];
    return {nodes,edges,focus_id:focusId,selected_id:selected?.id||null,clusterCount:cluster,
      totalVideos:inScope.size,totalChannels:ranked.length,visibleChannels:included.length,
      hiddenChannels:Math.max(0,ranked.length-included.length),captures:captures.length,
      expandedVideos:expansionVideos.length,truncated:!!graph.truncated,
      index:rel,selected,captureNodes:captures};
  }

  // A bounded lazy import; the exact same MIT-licensed ESM artifact is embedded
  // by the Rust ui-assets crate. Rendering works fully offline.
  let cytoscapeModule;
  const graphEngine=()=>cytoscapeModule||=import('/ui/vendor/cytoscape.esm.min.mjs').then(m=>m.default);

  function mount(container,callbacks={}) {
    const doc=container.ownerDocument;
    const el=(tag,klass,text)=>{
      const n=doc.createElement(tag);if(klass)n.className=klass;
      if(text!==undefined)n.textContent=String(text);return n;
    };
    const btn=(title,fn)=>{const n=el('button',null,title);n.type='button';n.addEventListener('click',fn);return n;};
    const toolbar=el('div','feed-network-toolbar');
    const scopeLabel=el('label','feed-network-scope','View');
    const scopeControl=el('select');scopeControl.setAttribute('aria-label','Feed network scope');
    for(const [value,label] of [['selected','Selected capture'],['all','All loaded captures']]){
      const o=el('option',null,label);o.value=value;scopeControl.append(o);
    }
    scopeLabel.append(scopeControl);
    const search=el('input','feed-network-search');search.type='search';
    search.placeholder='Find channels or videos';search.setAttribute('aria-label','Search feed graph');
    const more=btn('Show more nodes',()=>{limit=Math.min(100,limit+20);rebuild();});
    const zoomOut=btn('−',()=>setZoom(.8));zoomOut.setAttribute('aria-label','Zoom graph out');
    const zoomIn=btn('+',()=>setZoom(1.25));zoomIn.setAttribute('aria-label','Zoom graph in');
    const fit=btn('Fit',()=>fitGraph());
    toolbar.append(scopeLabel,search,more,zoomOut,zoomIn,fit);
    const stage=el('div','feed-network-stage');
    const canvas=el('div','feed-network-canvas');
    const tooltip=el('div','feed-network-tooltip');tooltip.hidden=true;
    const inspector=el('aside','feed-network-inspector');inspector.hidden=true;
    inspector.setAttribute('aria-label','Selected feed connection');
    const close=btn('×',()=>{inspector.hidden=true;focused=null;rebuild();});
    close.className='feed-network-inspector-close';close.setAttribute('aria-label','Close connection details');
    const detail=el('div','feed-network-detail');
    inspector.append(close,detail);
    stage.append(canvas,tooltip,inspector);
    const caption=el('p','feed-network-caption');caption.setAttribute('role','status');
    const legend=el('div','feed-network-legend');
    for(const [kind,text] of [['snapshot','Capture'],['channel','Channel'],['video','Video']]){
      const item=el('span','feed-network-legend-'+kind,text);legend.append(item);
    }
    container.replaceChildren(toolbar,stage,caption,legend);
    let cy=null,prepared=null,graph={nodes:[],edges:[]},captureId=null,scope='selected',
      focused=null,limit=42,query='',disposed=false,generation=0,labelsFrame=null,resizeFrame=null;
    const views=new Map(),MAX_VIEWS=6;
    const cacheKey=()=>JSON.stringify([scope,captureId,focused,query,limit]);
    let currentKey=null;
    const css=(variable,fallback)=>{
      const value=doc.defaultView?.getComputedStyle?.(doc.documentElement)?.getPropertyValue(variable);
      return value?.trim()||fallback;
    };
    const palette=()=>({
      bg:css('--bg','#101116'),text:css('--text','#e9e9ee'),
      muted:css('--muted','#9ca6b0'),accent:css('--accent','#e36b78'),
      channel:css('--ok','#4ca4b5'),video:css('--warn','#d5a061')
    });
    function saveView(){
      if(!cy||!currentKey)return;
      const positions=Object.fromEntries(cy.nodes().map(n=>[n.id(),n.position()]));
      views.delete(currentKey);
      views.set(currentKey,{positions,zoom:cy.zoom(),pan:cy.pan()});
      while(views.size>MAX_VIEWS)views.delete(views.keys().next().value);
    }
    function clearCanvas(){if(cy){saveView();cy.destroy();cy=null;}canvas.replaceChildren();tooltip.hidden=true;}
    function setZoom(factor){if(!cy)return;const z=Math.max(.25,Math.min(4,cy.zoom()*factor));
      cy.zoom({level:z,renderedPosition:{x:cy.width()/2,y:cy.height()/2}});updateLabels();}
    function fitGraph(){if(!cy)return;cy.fit(undefined,54);
      if(cy.zoom()>1.45){cy.zoom(1.45);cy.center();}updateLabels();}
    let updateLabels=()=>{};
    function list(parent,title,items,click,cap=18){
      const h=el('h4',null,title);parent.append(h);
      const ul=el('ul','feed-network-detail-list');
      for(const n of items.slice(0,cap)){
        const li=el('li');const b=btn(n.label,()=>click(n));b.title=n.label;
        li.append(b);ul.append(li);
      }
      parent.append(ul);
      if(items.length>cap)parent.append(el('p','feed-network-excess',
        (items.length-cap)+' more. Search or select a connection to explore them.'));
    }
    const message=(text,klass)=>detail.append(el('p',klass||'',text));
    function inspect(node,edge=null) {
      detail.replaceChildren();inspector.hidden=false;
      if(edge){
        detail.append(el('h3',null,'Recorded connection'));
        message(edge.reason||'Recorded connection in a captured feed.');
        const a=prepared?.index.nodes.get(edge.source),b=prepared?.index.nodes.get(edge.target);
        message((a?.label||edge.source)+' ↔ '+(b?.label||edge.target),'feed-network-emphasis');
        return;
      }
      if(!node)return;
      detail.append(el('h3',null,node.label));
      if(node.kind==='snapshot'){
        message('Captured feed · '+(node.count||0)+' recorded videos');
        message(node.source_url?'Source: '+node.source_url:'The original source URL was not recorded.');
        if(node.captured_at){
          const raw=Number(node.captured_at),d=new Date(raw>1e12?raw:raw*1000);
          if(!Number.isNaN(d.valueOf()))message('Captured '+d.toLocaleString());
        }
        detail.append(btn('View video cards',()=>callbacks.onOpen?.(node)));
        const matches=prepared?.nodes.filter(n=>n.kind==='channel')||[];
        list(detail,'Channels in this view',matches.slice(0,40),n=>focusNode(n.id),20);
      }else if(node.kind==='channel'){
        message(node.channel_id?'Channel ID: '+node.channel_id:
          'Channel ID not recorded; name-only grouping is limited to its source capture.');
        const vids=[...(prepared?.index.channelVideos.get(node.id)||[])].map(id=>prepared.index.nodes.get(id)).filter(Boolean);
        const relevant=vids.filter(v=>prepared?.index.videoCaptures.get(v.id)?.size>0);
        message(relevant.length+' recorded videos linked to this channel.');
        if(node.channel_id)detail.append(btn('Open channel',()=>callbacks.onOpen?.(node)));
        list(detail,'Recorded videos',relevant.sort((a,b)=>a.label.localeCompare(b.label)),
          n=>focusNode(n.id),40);
      }else if(node.kind==='video'){
        message('Recorded video · '+node.video_id);
        detail.append(btn('Play in Muninn',()=>callbacks.onOpen?.(node)));
        list(detail,'Appears in captured feeds',
          [...(prepared?.index.videoCaptures.get(node.id)||[])].map(id=>prepared.index.nodes.get(id)).filter(Boolean),
          n=>focusNode(n.id),20);
        list(detail,'Recorded channel',
          [...(prepared?.index.videoChannels.get(node.id)||[])].map(id=>prepared.index.nodes.get(id)).filter(Boolean),
          n=>focusNode(n.id),20);
      }
    }
    function readableLabels(){
      if(!cy)return;
      const z=cy.zoom(),width=cy.width(),height=cy.height(),theme=palette();
      cy.nodes().style({'font-size':11/z,'text-max-width':150/z,
        'text-outline-width':1.5/z,'text-margin-y':7/z});
      cy.nodes().removeClass('label-visible');
      const priority=cy.nodes().filter(n=>n.hasClass('active')||n.hasClass('adjacent')||
        n.data('landmark')||(z>1.45&&n.data('degree')>1))
        .sort((a,b)=>Number(b.hasClass('active'))-Number(a.hasClass('active'))||
          Number(b.hasClass('adjacent'))-Number(a.hasClass('adjacent'))||
          Number(b.data('hub'))-Number(a.data('hub'))||
          (b.data('degree')||0)-(a.data('degree')||0));
      const boxes=[];let shown=0;
      for(const n of priority){
        if(shown>=18)break;
        const label=String(n.data('label')||'');
        const p=n.renderedPosition(),r=Math.max(6,n.renderedOuterWidth()/2),
          w=Math.min(150,Math.max(32,label.length*6.3)),h=17;
        const candidates=[
          {x:p.x-w/2,y:p.y+r+7,v:'bottom',a:'center',dx:0,dy:8},
          {x:p.x-w/2,y:p.y-r-7-h,v:'top',a:'center',dx:0,dy:-7},
          {x:p.x+r+9,y:p.y-h/2,v:'center',a:'right',dx:9,dy:0},
          {x:p.x-r-9-w,y:p.y-h/2,v:'center',a:'left',dx:-9,dy:0}
        ];
        const chosen=candidates.find(p=>p.x>4&&p.y>4&&p.x+w<width-4&&p.y+h<height-4&&
          !boxes.some(b=>p.x<b.x+b.w+6&&p.x+w+6>b.x&&p.y<b.y+b.h+5&&p.y+h+5>b.y));
        if(chosen){
          boxes.push({...chosen,w,h});shown++;n.addClass('label-visible');
          n.style({'text-valign':chosen.v,'text-halign':chosen.a,
            'text-margin-x':chosen.dx/z,'text-margin-y':chosen.dy/z});
        }
      }
    }
    updateLabels=readableLabels;
    function chooseNode(node){
      if(!cy||!node?.length)return;
      cy.elements().removeClass('muted active adjacent');
      const near=node.closedNeighborhood();
      cy.elements().difference(near).addClass('muted');
      node.addClass('active');near.nodes().difference(node).addClass('adjacent');
      near.edges().addClass('active');updateLabels();
    }
    function focusNode(id){
      const record=prepared?.index.nodes.get(id);
      if(!record)return;
      focused=id;
      if(record.kind==='snapshot'&&captureId!==id){
        captureId=id;scope='selected';scopeControl.value=scope;
        callbacks.onSelect?.(record);
      }
      rebuild();
      inspect(record);
    }
    function showDetails(record){if(!record)return;inspect(record);}
    function copyContext(){
      const snap=prepared?.selected;
      caption.textContent=(prepared?.totalVideos||0)+' unique videos · '+
        (prepared?.captures||0)+' capture(s) · '+(prepared?.visibleChannels||0)+' of '+
        (prepared?.totalChannels||0)+' channel groups displayed'+
        (prepared?.hiddenChannels?' · Search or show more to explore the rest':'')+
        (prepared?.truncated?' · Some historical items were capped':'');
      more.hidden=!prepared?.hiddenChannels||limit>=100;
    }
    function layoutGraph(cytoscape,model,key,token){
      const prior=views.get(key),theme=palette();
      const colors={snapshot:theme.accent,channel:theme.channel,video:theme.video};
      const elements=model.nodes.map(n=>({data:{...n,fullLabel:n.label,
        label:n.label.length>43?n.label.slice(0,42)+'…':n.label,
        color:colors[n.kind],size:n.kind==='snapshot'?28:n.kind==='video'?11:Math.min(28,13+Math.sqrt(n.count||1)*3)},
        position:prior?.positions[n.id],
        classes:[n.kind,n.hub?'hub':'',n.landmark?'landmark':''].join(' ')}));
      for(const edge of model.edges)elements.push({data:edge,classes:edge.basis});
      cy=cytoscape({container:canvas,elements,minZoom:.25,maxZoom:4,
        wheelSensitivity:.23,selectionType:'single',
        style:[
          {selector:'node',style:{shape:'ellipse',width:'data(size)',height:'data(size)',
            'background-color':'data(color)','border-width':1.7,'border-color':theme.bg,
            label:'','font-family':'sans-serif','font-size':11,color:theme.muted,
            'text-valign':'bottom','text-halign':'center','text-margin-y':7,
            'text-wrap':'ellipsis','text-max-width':150,
            'text-outline-color':theme.bg,'text-outline-width':2,'overlay-padding':10,
            'text-events':'yes'}},
          {selector:'node.snapshot',style:{shape:'diamond',width:28,height:28}},
          {selector:'node.video',style:{shape:'round-rectangle',width:13,height:12}},
          {selector:'node.hub',style:{'border-width':2.5,'border-color':theme.text}},
          {selector:'node.label-visible,node.hovered',style:{label:'data(label)'}},
          {selector:'node.active',style:{label:'data(label)','border-width':3.5,
            'border-color':theme.accent,color:theme.text,'font-weight':600,
            'text-background-color':theme.bg,'text-background-opacity':.96,
            'text-background-padding':4}},
          {selector:'node.adjacent',style:{color:theme.text}},
          {selector:'edge',style:{width:'mapData(count,1,20,1,3)','line-color':theme.muted,
            'curve-style':'bezier',opacity:.29,'overlay-padding':6}},
          {selector:'edge.captured_in',style:{'line-color':theme.channel,opacity:.35}},
          {selector:'edge.published',style:{'line-color':theme.video,opacity:.6}},
          {selector:'.muted',style:{opacity:.10}},
          {selector:'edge.active',style:{width:2.8,'line-color':theme.accent,opacity:.93}}
        ],layout:{name:'preset',fit:false}});
      currentKey=key;
      cy.on('tap','node',event=>focusNode(event.target.id()));
      cy.on('dbltap','node',event=>{
        const record=prepared?.index.nodes.get(event.target.id());
        if(record)callbacks.onOpen?.(record);
      });
      cy.on('tap','edge',event=>{
        cy.elements().removeClass('muted active adjacent');event.target.addClass('active');
        inspect(null,event.target.data());updateLabels();
      });
      cy.on('tap',event=>{if(event.target!==cy)return;
        cy.elements().removeClass('muted active adjacent');inspector.hidden=true;focused=null;updateLabels();});
      cy.on('mouseover','node',event=>{
        const n=event.target,p=n.renderedPosition();n.addClass('hovered');
        tooltip.textContent=n.data('fullLabel');tooltip.hidden=false;
        tooltip.style.left=Math.max(8,Math.min(stage.clientWidth-240,p.x+16))+'px';
        tooltip.style.top=Math.max(8,p.y-32)+'px';
      });
      cy.on('mouseout','node',event=>{event.target.removeClass('hovered');tooltip.hidden=true;});
      cy.on('viewport',()=>{
        if(labelsFrame)cancelAnimationFrame(labelsFrame);
        labelsFrame=requestAnimationFrame(readableLabels);
      });
      canvas.setAttribute('role','application');canvas.tabIndex=0;
      canvas.setAttribute('aria-label','Feed connections map. Arrow keys inspect, Enter expands the selected node, double-click opens it.');
      canvas.onkeydown=e=>{
        if(!cy||!['ArrowRight','ArrowDown','ArrowLeft','ArrowUp','Enter'].includes(e.key))return;
        const nodes=cy.nodes().sort((a,b)=>a.id().localeCompare(b.id()));
        if(!nodes.length)return;e.preventDefault();
        const current=nodes.findIndex(n=>n.hasClass('active'));
        if(e.key==='Enter'){focusNode(nodes[Math.max(0,current)].id());return;}
        const d=e.key==='ArrowLeft'||e.key==='ArrowUp'?-1:1;
        const next=nodes[(current+d+nodes.length)%nodes.length];
        focusNode(next.id());cy.animate({center:{eles:next}},{duration:180});
      };
      cy.resize();
      if(prior){cy.viewport({zoom:prior.zoom,pan:prior.pan});
        if(focused){const target=cy.getElementById(focused);if(target.length)chooseNode(target);}
        updateLabels();return;
      }
      const instance=cy;
      const layout=cy.layout({name:'cose',animate:false,randomize:false,padding:52,
        nodeRepulsion:()=>12500,idealEdgeLength:()=>125,edgeElasticity:()=>85,
        gravity:.65,componentSpacing:100,numIter:360,nodeDimensionsIncludeLabels:false});
      layout.one('layoutstop',()=>{if(disposed||cy!==instance||generation!==token)return;
        fitGraph();if(focused){const n=cy.getElementById(focused);if(n.length)chooseNode(n);}
        updateLabels();});
      layout.run();
    }
    async function rebuild(){
      if(disposed)return;
      const token=++generation;
      prepared=prepareNetwork(graph,{snapshotId:captureId,scope,query,focusId:focused,maxChannels:limit});
      copyContext();
      clearCanvas();
      if(!prepared.nodes.length){canvas.append(el('p','feed-network-empty','No recorded connections for this capture.'));return;}
      canvas.append(el('p','feed-network-loading','Arranging captured connections…'));
      try{
        const cytoscape=await graphEngine();
        if(disposed||token!==generation)return;
        canvas.replaceChildren();
        const key=cacheKey();
        layoutGraph(cytoscape,prepared,key,token);
      }catch(error){
        if(disposed||token!==generation)return;
        canvas.replaceChildren(el('p','feed-network-error','Graph could not load. Your captured feeds are still available in Cards.'));
        caption.textContent='Graph engine unavailable; nothing was changed in your saved captures.';
      }
    }
    scopeControl.addEventListener('change',()=>{scope=scopeControl.value;focused=null;limit=42;rebuild();});
    search.addEventListener('input',()=>{query=search.value;focused=null;rebuild();});
    const observer=typeof ResizeObserver==='function'?new ResizeObserver(()=>{
      if(resizeFrame)cancelAnimationFrame(resizeFrame);
      resizeFrame=requestAnimationFrame(()=>{cy?.resize();updateLabels();});
    }):null;
    observer?.observe(canvas);
    return {
      render(data,initialId){
        graph=data||{nodes:[],edges:[]};
        if(initialId&&graph.nodes?.some(n=>n.id===initialId&&n.kind==='snapshot'))captureId=initialId;
        else if(!graph.nodes?.some(n=>n.id===captureId))captureId=graph.nodes?.find(n=>n.kind==='snapshot')?.id||null;
        focused=null;return rebuild();
      },
      select(id){
        if(!id||id===captureId)return;
        if(graph.nodes?.some(n=>n.id===id&&n.kind==='snapshot')){
          captureId=id;focused=null;rebuild();
        }
      },
      search(value){search.value=String(value||'');query=search.value;rebuild();},
      zoom:setZoom,fit:fitGraph,
      destroy(){
        disposed=true;generation++;observer?.disconnect();
        if(labelsFrame)cancelAnimationFrame(labelsFrame);
        if(resizeFrame)cancelAnimationFrame(resizeFrame);
        clearCanvas();container.replaceChildren();
      }
    };
  }

  const api={build,positions,overview,relationships,prepareNetwork,mount};
  if(typeof module!=='undefined'&&module.exports)module.exports=api;
  if(root)root.MuninnFeedGraph=api;
})(typeof window!=='undefined'?window:null);
