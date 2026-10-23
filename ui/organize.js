/* Named tab groups and library collections; the CLI uses the same organization API. */
(function () {
  'use strict';
  const M = window.Muninn, L = M.L, h = M.h;
  M.organizationPanel = (section) => {
    let active = false;
    return {
    mount(el) {
      active = true; el.classList.add('page');
      const lists = h('div'), selected = new Set();
      const name = h('input', { type: 'text', placeholder: 'Name', 'aria-label': 'New organization name', maxlength: '80' });
      const kind = h('select', { 'aria-label': 'Organization type' }, ...(section==='tabs' ? [['tab_group','Tab group']] : [['collection','Collection'],['tag','Tag']]).map(([value,text])=>h('option',{value,text})));
      const target = h('select', { 'aria-label': 'Move selected tabs to group' });
      const search = h('input', { type: 'search', placeholder: 'Find tabs', 'aria-label': 'Find tabs', oninput: () => render() });
      async function run(change) { try { await M.organize(change); if (active) render(); return true; } catch (e) { M.flashError(e); return false; } }
      function render() {
        const org = M.organization;
        const previous = target.value;
        target.replaceChildren(h('option', { value: '', text: 'Select target group' }), ...Object.keys(org.tab_groups).map((value) => h('option', { value, text: value })));
        target.value = previous;
        const sections = [];
        const groupOf = (id) => Object.keys(org.tab_groups).find((name) => org.tab_groups[name].tab_ids.includes(String(id))) || '';
        if(section==='tabs') for(const groupName of ['',...Object.keys(org.tab_groups)]) {
          const group=org.tab_groups[groupName];
          const tabs=M.orderedTabs().filter(t=>groupOf(t.id)===groupName && (L.tabTitle(t)+' '+t.url).toLowerCase().includes(search.value.toLowerCase()));
          const rows=h('div',{class:'organize-tabs'});
          for(const tab of tabs) {
            const choose=h('select',{'aria-label':'Group for '+L.tabTitle(tab)},h('option',{value:'',text:'Ungrouped'}),...Object.keys(org.tab_groups).map(value=>h('option',{value,text:value})));
            choose.value=groupName;
            choose.addEventListener('change',()=>run({kind:'tab',action:'move',name:choose.value,ids:[String(tab.id)]}));
            const row=h('div',{class:'organize-tab','data-tab-id':tab.id},
              h('input',{type:'checkbox',checked:selected.has(tab.id),'aria-label':'Select '+L.tabTitle(tab),onchange:event=>{if(event.target.checked)selected.add(tab.id);else selected.delete(tab.id);}}),
              h('button',{class:'link',text:L.tabTitle(tab),title:'Drag to reorder or click to open',onclick:()=>M.activateTab(tab.id)}),choose,
              h('button',{text:'Close',onclick:async()=>{await M.closeTab(tab.id);selected.delete(tab.id);render();}}));
            M.dragTab(row,tab.id);M.dropTabs(row,groupName,()=>String(tab.id),render);rows.append(row);
          }
          if(!tabs.length) rows.append(h('p',{class:'muted drop-hint',text:'Drop tabs here'}));
          const controls=h('div',{class:'actions'});
          if(group) {
            const rename=h('input',{value:groupName,'aria-label':'Rename tab group '+groupName});
            controls.append(rename,h('button',{text:'Rename',onclick:()=>run({kind:'tab_group',action:'rename',name:groupName,new_name:rename.value.trim()})}),
              h('button',{text:group.collapsed?'Expand group':'Collapse group',onclick:()=>run({kind:'tab_group',action:'collapse',name:groupName,collapsed:!group.collapsed})}),
              h('button',{text:'Remove group',onclick:()=>run({kind:'tab_group',action:'delete',name:groupName})}));
          }
          const card=h('section',{class:'card tab-group-card','data-group':groupName,'aria-label':groupName?'Tab group '+groupName:'Ungrouped tabs'},h('h3',{text:(groupName||'Ungrouped')+' · '+tabs.length}),rows,controls);
          M.dropTabs(card,groupName,null,render);sections.push(card);
        }
        if(section!=='tabs') for (const [setKind, sets] of [['collection', org.collections], ['tag', org.tags || {}]]) for (const [collection, ids] of Object.entries(sets)) {
          const rename = h('input', { value: collection, 'aria-label': 'Rename ' + setKind + ' ' + collection });
          sections.push(h('section', { class: 'card' }, h('h2', { text: collection + ' · ' + ids.length + ' videos' }), h('div', { class: 'actions' }, rename,
            h('button', { text: 'Rename', onclick: () => run({ kind: setKind, action: 'rename', name: collection, new_name: rename.value.trim() }) }),
            h('button', { text: 'Remove ' + setKind, onclick: () => run({ kind: setKind, action: 'delete', name: collection }) })),
            h('p', { class: 'muted', text: 'Assign videos in the Videos section. Removing this ' + setKind + ' keeps its videos.' })));
        }
        lists.replaceChildren(...sections);
      }
      async function refresh() { try { await M.refreshOrganization(); await M.refreshTabs(); if (active) render(); } catch (e) { M.flashError(e); } }
      el.append(h('h2', { text: section==='tabs' ? 'Tab groups' : 'Collections & tags' }), h('p', { class: 'muted', text: section==='tabs' ? 'Group, collapse and bulk move your open tabs.' : 'A video can belong to several collections and have several tags.' }),
        h('div', { class: 'actions' }, name, kind, h('button', { text: 'Create', onclick: async () => { if (await run({ kind: kind.value, action: 'create', name: name.value.trim() })) name.value = ''; } }), h('button', { text: 'Refresh', onclick: refresh })),
        section==='tabs' ? h('div', { class: 'actions' }, search, target, h('button', { text: 'Move selected tabs', onclick: async () => { if (target.value && selected.size && await run({ kind: 'tab_group', action: 'assign', name: target.value, ids: Array.from(selected, String) })) { selected.clear(); render(); } } })) : null, lists);
      M.refreshLibraryGroups=render;
      refresh();
    },
    unmount() { active = false; M.refreshLibraryGroups=null; },
    };
  };
})();
