// Harness-only Tauri boundary, no native API calls or config reads.
window.__fixtureReady=(async()=>{
const state=await (await fetch('/__fixture/desktop-state'+location.search)).json();
let settings={paths:{instruction_files:['/fixture/instructions.md'],skill_roots:['/fixture/skills']},plugin_ids:['fixture-plugin'],target:{config_path:'/fixture/runner.toml',client_id:'fixture-runner',server_url:'http://127.0.0.1:1'},can_restart:true};
let permissions={supported:true,foreground:true,desktop_accessibility:false,desktop_screen_recording:false};
window.__fixtureCalls=[];let callbackId=1;const callbacks=new Map();
window.__TAURI_INTERNALS__={transformCallback(fn){const id=callbackId++;callbacks.set(id,fn);return id},unregisterCallback(id){callbacks.delete(id)},convertFileSrc(){throw Error('Unexpected file access')},async invoke(cmd,args={}){
 window.__fixtureCalls.push({cmd,args:structuredClone(args)});
 switch(cmd){
 case 'plugin:event|listen':return callbackId++;
 case 'plugin:event|unlisten':return null;
 case 'get_desktop_state':case 'refresh_runtime_status':case 'observe_chatgpt_activity':case 'resume_saved_runtime':case 'restart_owned_runner':return structuredClone(state);
 case 'get_bounded_activity':return [];
 case 'get_launch_at_login':return false;
 case 'set_launch_at_login':return args.request.enabled;
 case 'get_runner_settings':return structuredClone(settings);
 case 'add_runner_plugin':settings.plugin_ids.push(args.request.provider.id);return structuredClone(state);
 case 'update_runner_settings':settings.paths=structuredClone(args.request.paths);return structuredClone(state);
 case 'get_computer_permissions':return {...permissions};
 case 'request_computer_permission':if(args.action==='accessibility')permissions.desktop_accessibility=true;if(args.action==='screen_recording')permissions.desktop_screen_recording=true;return {...permissions};
 case 'update_tunnel_config':if(args.request.action==='save'){state.openai_tunnel_config.saved_tunnel_id=args.request.tunnelId;state.openai_tunnel_config.effective_tunnel_id=args.request.tunnelId;}return structuredClone(state);
 case 'activate_local_project':{let project=state.saved_projects.find(p=>p.path===args.request.projectPath);if(!project){project={path:args.request.projectPath,allowed_root:args.request.projectPath,is_git_repository:true,runtime_project_id:'agent:fixture-runner:gamma'};state.saved_projects.push(project);}state.project=project;return structuredClone(state);}
 case 'plugin:dialog|open':return '/fixture/gamma';
 case 'inspect_project':return {path:args.request.projectPath,allowed_root:args.request.projectPath,is_git_repository:true,runtime_project_id:'agent:fixture-runner:gamma'};
 case 'configure_local_setup':state.project={path:args.request.projectPath,allowed_root:args.request.projectPath,is_git_repository:true,runtime_project_id:'agent:fixture-runner:gamma'};state.saved_projects.push(state.project);return structuredClone(state);
 case 'plugin:clipboard-manager|write_text':return null;
 default:throw Error('Unimplemented Tauri fixture '+cmd);
 }
}};
window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener(){}};
localStorage.setItem('webcodex.desktop.locale','en-US');
if(!new URLSearchParams(location.search).has('permissions'))localStorage.setItem('desktop-permissions-explained','1');else localStorage.removeItem('desktop-permissions-explained');
window.dispatchEvent(new Event('fixture-ready'));
})();
