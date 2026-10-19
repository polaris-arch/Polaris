import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it, vi } from 'vitest';
vi.mock('react-i18next',()=>({useTranslation:()=>({t:(key:string)=>key})}));
import { IdentityRepairFields } from './IdentityRepairFields';
import type { useIdentityRepair } from './use-identity-repair';
it('renders every conflicting record with no default radio or confirmation',()=>{
 const form: ReturnType<typeof useIdentityRepair> = {
  preview:{version:'v',groups:[{indices:[243,244],repairable:true,records:[
   {index:243,name:'First',protocol:'trojan',address:'a.invalid',port:443},
   {index:244,name:'Second <node>',protocol:'trojan',address:'b.invalid',port:443},
  ]}]}, keepers:{},setKeepers:vi.fn(),acknowledged:false,setAcknowledged:vi.fn(),busy:false,message:null,done:false,load:vi.fn(),retryRefresh:vi.fn(),save:vi.fn(),canSave:false,
 };
 const html=renderToStaticMarkup(<IdentityRepairFields form={form}/>);
 expect(html.match(/type="radio"/g)).toHaveLength(2);
 expect(html).not.toContain(' checked');
 expect(html).toContain('Second &lt;node&gt;');
 expect(html).toContain('identityRepair.confirmReferences');
 expect(html).not.toContain('version');
});
