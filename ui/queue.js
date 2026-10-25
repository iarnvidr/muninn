/* Bounded playlist queue navigation. */
(function(root,factory){'use strict';const q=factory();if(typeof module==='object'&&module.exports)module.exports=q;else root.MuninnQueue=q;})(typeof self!=='undefined'?self:this,function(){'use strict';
 function next(items,index,direction){const step=direction<0?-1:1;for(let i=index+step;i>=0&&i<items.length;i+=step)if(items[i]&&items[i].available!==false)return i;return null;}
 function selection(items,index){return items[index]||null;}
 return {next,selection};
});
