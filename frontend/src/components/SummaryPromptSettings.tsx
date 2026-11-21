'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ArrowDown, ArrowUp, Plus, RotateCcw, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

/** Mirrors `SummaryInstructions` in src-tauri/src/summary/instructions.rs */
interface SummaryInstructions {
  content: string;
  is_custom: boolean;
  default_content: string;
}

interface TemplateInfo {
  id: string;
  name: string;
  description: string;
}

/** Mirrors `TemplateSource` in src-tauri/src/summary/template_commands.rs */
interface TemplateSource {
  id: string;
  content: string;
  is_custom: boolean;
  default_content: string | null;
}

type SectionFormat = 'paragraph' | 'list' | 'string';

interface TemplateSection {
  title: string;
  instruction: string;
  format: SectionFormat;
  item_format?: string;
  example_item_format?: string;
}

interface TemplateDraft {
  name: string;
  description: string;
  sections: TemplateSection[];
}

const AUTO_SUMMARY_TEMPLATE_ID = 'standard_meeting';

const FORMAT_LABELS: Record<SectionFormat, string> = {
  paragraph: 'Paragraph',
  list: 'List',
  string: 'Short text',
};

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function parseTemplate(json: string): TemplateDraft {
  const raw = JSON.parse(json);
  const sections: TemplateSection[] = Array.isArray(raw.sections)
    ? raw.sections.map((section: Record<string, unknown>) => ({
        ...section,
        title: String(section.title ?? ''),
        instruction: String(section.instruction ?? ''),
        format: (['paragraph', 'list', 'string'].includes(String(section.format))
          ? section.format
          : 'paragraph') as SectionFormat,
      }))
    : [];
  return {
    name: String(raw.name ?? ''),
    description: String(raw.description ?? ''),
    sections,
  };
}

function serializeTemplate(draft: TemplateDraft): string {
  return JSON.stringify(
    {
      name: draft.name.trim(),
      description: draft.description.trim(),
      sections: draft.sections.map(({ item_format, example_item_format, ...section }) => ({
        ...section,
        title: section.title.trim(),
        instruction: section.instruction.trim(),
        ...(item_format?.trim() ? { item_format: item_format.trim() } : {}),
        ...(example_item_format?.trim() ? { example_item_format: example_item_format.trim() } : {}),
      })),
    },
    null,
    2
  );
}

function StatusBadge({ custom }: { custom: boolean }) {
  return custom ? (
    <span className="rounded-full bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-800">Customized</span>
  ) : (
    <span className="rounded-full bg-gray-100 px-2 py-0.5 text-xs font-medium text-gray-600">Default</span>
  );
}

/**
 * Settings → Prompts: edit the base instructions and the summary templates.
 * Both are stored as overrides in the app data folder and apply to the next summary.
 */
export function SummaryPromptSettings() {
  return (
    <div className="space-y-6">
      <div>
        <h3 className="mb-2 text-lg font-semibold">Summary Prompts</h3>
        <p className="text-sm text-gray-600">
          Control how summaries are written. Changes apply to the next summary, including the automatic one
          after a recording.
        </p>
      </div>
      <BaseInstructionsEditor />
      <TemplateEditor />
    </div>
  );
}

function BaseInstructionsEditor() {
  const [saved, setSaved] = useState<SummaryInstructions | null>(null);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);

  const apply = useCallback((instructions: SummaryInstructions) => {
    setSaved(instructions);
    setDraft(instructions.content);
  }, []);

  useEffect(() => {
    invoke<SummaryInstructions>('api_get_summary_instructions')
      .then(apply)
      .catch((error) => toast.error('Could not load summary instructions', { description: errorMessage(error) }));
  }, [apply]);

  const dirty = saved !== null && draft.trim() !== saved.content.trim();

  const run = async (command: string, args: Record<string, unknown>, success: (r: SummaryInstructions) => string) => {
    setBusy(true);
    try {
      const result = await invoke<SummaryInstructions>(command, args);
      apply(result);
      toast.success(success(result));
    } catch (error) {
      toast.error('Could not save summary instructions', { description: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-3 rounded-lg border p-4">
      <div className="flex items-center gap-2">
        <div className="font-medium">Base instructions</div>
        {saved && <StatusBadge custom={saved.is_custom} />}
      </div>
      <p className="text-sm text-gray-600">
        The general rules the AI follows when it writes the summary. Rule 1 is fixed and always comes first: write
        in English (Meetily translates to your summary language afterwards). The template&apos;s section
        instructions are added after these rules.
      </p>
      <Textarea
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        rows={10}
        spellCheck={false}
        className="font-mono text-xs"
        disabled={!saved || busy}
        aria-label="Base instructions"
      />
      <p className="text-xs text-gray-500">
        Example: add <code>9. Be brief: at most 3 bullet points per section and no filler.</code>
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          disabled={!dirty || busy}
          onClick={() =>
            run('api_save_summary_instructions', { content: draft }, (r) =>
              r.is_custom ? 'Base instructions saved' : 'Default instructions restored'
            )
          }
        >
          Save
        </Button>
        <Button size="sm" variant="outline" disabled={!dirty || busy} onClick={() => saved && setDraft(saved.content)}>
          Discard changes
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!saved?.is_custom || busy}
          onClick={() => run('api_reset_summary_instructions', {}, () => 'Default instructions restored')}
        >
          <RotateCcw className="mr-1 h-3.5 w-3.5" />
          Reset to default
        </Button>
      </div>
    </div>
  );
}

function TemplateEditor() {
  const [templates, setTemplates] = useState<TemplateInfo[]>([]);
  const [selectedId, setSelectedId] = useState<string>(AUTO_SUMMARY_TEMPLATE_ID);
  const [source, setSource] = useState<TemplateSource | null>(null);
  const [draft, setDraft] = useState<TemplateDraft | null>(null);
  const [savedJson, setSavedJson] = useState('');
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const loadTemplates = useCallback(async () => {
    try {
      setTemplates(await invoke<TemplateInfo[]>('api_list_templates'));
    } catch (error) {
      toast.error('Could not load templates', { description: errorMessage(error) });
    }
  }, []);

  const loadSource = useCallback(async (templateId: string) => {
    setLoadError(null);
    try {
      const result = await invoke<TemplateSource>('api_get_template_source', { templateId });
      setSource(result);
      try {
        const parsed = parseTemplate(result.content);
        setDraft(parsed);
        setSavedJson(serializeTemplate(parsed));
      } catch (error) {
        setDraft(null);
        setLoadError(`This template file is not valid JSON: ${errorMessage(error)}`);
      }
    } catch (error) {
      setSource(null);
      setDraft(null);
      setLoadError(errorMessage(error));
    }
  }, []);

  useEffect(() => {
    void loadTemplates();
  }, [loadTemplates]);

  useEffect(() => {
    void loadSource(selectedId);
  }, [selectedId, loadSource]);

  const dirty = draft !== null && serializeTemplate(draft) !== savedJson;

  const selectTemplate = (templateId: string) => {
    if (dirty && !window.confirm('Discard your unsaved template changes?')) return;
    setSelectedId(templateId);
  };

  const updateDraft = (patch: Partial<TemplateDraft>) => setDraft((current) => (current ? { ...current, ...patch } : current));

  const updateSection = (index: number, patch: Partial<TemplateSection>) =>
    setDraft((current) =>
      current
        ? { ...current, sections: current.sections.map((section, i) => (i === index ? { ...section, ...patch } : section)) }
        : current
    );

  const moveSection = (index: number, offset: -1 | 1) =>
    setDraft((current) => {
      if (!current) return current;
      const target = index + offset;
      if (target < 0 || target >= current.sections.length) return current;
      const sections = [...current.sections];
      [sections[index], sections[target]] = [sections[target], sections[index]];
      return { ...current, sections };
    });

  const removeSection = (index: number) =>
    setDraft((current) => (current ? { ...current, sections: current.sections.filter((_, i) => i !== index) } : current));

  const addSection = () =>
    setDraft((current) =>
      current
        ? { ...current, sections: [...current.sections, { title: '', instruction: '', format: 'paragraph' }] }
        : current
    );

  const save = async () => {
    if (!draft) return;
    setBusy(true);
    try {
      await invoke('api_save_custom_template', { templateId: selectedId, templateJson: serializeTemplate(draft) });
      toast.success('Template saved', { description: 'It will be used for the next summary.' });
      await Promise.all([loadSource(selectedId), loadTemplates()]);
    } catch (error) {
      toast.error('Could not save template', { description: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  };

  const reset = async () => {
    if (!window.confirm('Restore the original template? Your changes to it will be removed.')) return;
    setBusy(true);
    try {
      await invoke('api_reset_custom_template', { templateId: selectedId });
      toast.success('Original template restored');
      await Promise.all([loadSource(selectedId), loadTemplates()]);
    } catch (error) {
      toast.error('Could not restore template', { description: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4 rounded-lg border p-4">
      <div className="flex flex-wrap items-center gap-3">
        <div className="font-medium">Template</div>
        <Select value={selectedId} onValueChange={selectTemplate} disabled={busy}>
          <SelectTrigger className="w-72">
            <SelectValue placeholder="Choose a template" />
          </SelectTrigger>
          <SelectContent>
            {templates.map((template) => (
              <SelectItem key={template.id} value={template.id}>
                {template.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {source && <StatusBadge custom={source.is_custom} />}
      </div>
      <p className="text-sm text-gray-600">
        Each section becomes a heading in the summary; its instruction tells the AI what to write there.
        {selectedId === AUTO_SUMMARY_TEMPLATE_ID && ' This template is used for the automatic summary after a recording.'}
      </p>

      {loadError && (
        <div className="rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800">
          {loadError}
          {source?.is_custom && source.default_content && (
            <Button size="sm" variant="outline" className="ml-3" onClick={reset} disabled={busy}>
              Restore original
            </Button>
          )}
        </div>
      )}

      {draft && (
        <>
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="space-y-1 text-sm">
              <span className="font-medium text-gray-700">Name</span>
              <Input value={draft.name} onChange={(event) => updateDraft({ name: event.target.value })} disabled={busy} />
            </label>
            <label className="space-y-1 text-sm">
              <span className="font-medium text-gray-700">Description</span>
              <Input
                value={draft.description}
                onChange={(event) => updateDraft({ description: event.target.value })}
                disabled={busy}
              />
            </label>
          </div>

          <div className="space-y-3">
            {draft.sections.map((section, index) => (
              <div key={index} className="space-y-2 rounded-md border bg-gray-50 p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="w-6 text-sm font-medium text-gray-500">{index + 1}.</span>
                  <Input
                    value={section.title}
                    onChange={(event) => updateSection(index, { title: event.target.value })}
                    placeholder="Section title"
                    className="min-w-0 flex-1 bg-white"
                    disabled={busy}
                    aria-label={`Section ${index + 1} title`}
                  />
                  <Select
                    value={section.format}
                    onValueChange={(value) => updateSection(index, { format: value as SectionFormat })}
                    disabled={busy}
                  >
                    <SelectTrigger className="w-36 bg-white" aria-label={`Section ${index + 1} format`}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {(Object.keys(FORMAT_LABELS) as SectionFormat[]).map((format) => (
                        <SelectItem key={format} value={format}>
                          {FORMAT_LABELS[format]}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    size="icon"
                    variant="ghost"
                    onClick={() => moveSection(index, -1)}
                    disabled={busy || index === 0}
                    aria-label="Move section up"
                  >
                    <ArrowUp className="h-4 w-4" />
                  </Button>
                  <Button
                    size="icon"
                    variant="ghost"
                    onClick={() => moveSection(index, 1)}
                    disabled={busy || index === draft.sections.length - 1}
                    aria-label="Move section down"
                  >
                    <ArrowDown className="h-4 w-4" />
                  </Button>
                  <Button
                    size="icon"
                    variant="ghost"
                    onClick={() => removeSection(index)}
                    disabled={busy || draft.sections.length <= 1}
                    aria-label="Remove section"
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
                <Textarea
                  value={section.instruction}
                  onChange={(event) => updateSection(index, { instruction: event.target.value })}
                  placeholder="What should the AI write in this section?"
                  rows={2}
                  className="bg-white text-sm"
                  disabled={busy}
                  aria-label={`Section ${index + 1} instruction`}
                />
                {(section.format === 'list' || section.item_format) && (
                  <label className="block space-y-1 text-xs">
                    <span className="text-gray-600">
                      Table header (optional): a Markdown table the AI fills with one row per item
                    </span>
                    <Textarea
                      value={section.item_format ?? ''}
                      onChange={(event) => updateSection(index, { item_format: event.target.value })}
                      placeholder="| **Owner** | Task | Due |&#10;| --- | --- | --- |"
                      rows={2}
                      spellCheck={false}
                      className="bg-white font-mono text-xs"
                      disabled={busy}
                    />
                  </label>
                )}
              </div>
            ))}
          </div>

          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" onClick={addSection} disabled={busy}>
              <Plus className="mr-1 h-3.5 w-3.5" />
              Add section
            </Button>
            <div className="flex-1" />
            <Button size="sm" onClick={save} disabled={!dirty || busy}>
              Save template
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => void loadSource(selectedId)}
              disabled={!dirty || busy}
            >
              Discard changes
            </Button>
            {source?.is_custom && source.default_content && (
              <Button size="sm" variant="outline" onClick={reset} disabled={busy}>
                <RotateCcw className="mr-1 h-3.5 w-3.5" />
                Reset to original
              </Button>
            )}
          </div>
        </>
      )}
    </div>
  );
}
