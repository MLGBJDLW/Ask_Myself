import { useMemo, useRef, useState } from 'react';
import { motion } from 'framer-motion';
import { Check, CheckCircle2, HelpCircle, Send } from 'lucide-react';
import { useTranslation } from '../../i18n';
import { formatQuestionResponse, type QuestionRequest } from '../../lib/questionCards';
import type { ArtifactPayload } from '../../types/conversation';
import { Button } from '../ui/Button';

interface QuestionRequestPanelProps {
  request: QuestionRequest;
  answered?: boolean;
  onSubmit?: (message: string, artifact: ArtifactPayload) => void;
}

export function QuestionRequestPanel({ request, answered = false, onSubmit }: QuestionRequestPanelProps) {
  const { t } = useTranslation();
  const [answers, setAnswers] = useState<Record<string, string[]>>({});
  const [otherAnswers, setOtherAnswers] = useState<Record<string, string>>({});
  const [submitted, setSubmitted] = useState(false);
  const submittedRef = useRef(false);
  const complete = useMemo(
    () => hasCompleteAnswers(request, answers),
    [answers, request],
  );
  const autoSubmitChoices = useMemo(
    () => request.questions.every((question) => (
      question.type === 'single_choice' || question.type === 'confirm'
    )),
    [request.questions],
  );
  const isAnswered = answered || submitted || request.status === 'answered';

  const submitAnswers = (nextAnswers: Record<string, string[]>) => {
    if (
      !hasCompleteAnswers(request, nextAnswers) ||
      isAnswered ||
      !onSubmit ||
      submittedRef.current
    ) {
      return;
    }
    submittedRef.current = true;
    setSubmitted(true);
    const response = formatQuestionResponse(request, nextAnswers);
    onSubmit(response.message, response.artifact);
  };

  const selectSingle = (id: string, value: string, submitWhenComplete = false) => {
    const nextAnswers = { ...answers, [id]: [value] };
    setAnswers(nextAnswers);
    if (submitWhenComplete && autoSubmitChoices) {
      submitAnswers(nextAnswers);
    }
  };

  const toggleMulti = (id: string, value: string) => {
    setAnswers((current) => {
      const selected = current[id] ?? [];
      return {
        ...current,
        [id]: selected.includes(value)
          ? selected.filter((answer) => answer !== value)
          : [...selected, value],
      };
    });
  };

  const setOther = (id: string, value: string, multi: boolean) => {
    const previous = otherAnswers[id] ?? '';
    setOtherAnswers((current) => ({ ...current, [id]: value }));
    setAnswers((current) => {
      const withoutPrevious = (current[id] ?? []).filter((answer) => answer !== previous);
      return {
        ...current,
        [id]: value.trim() ? (multi ? [...withoutPrevious, value] : [value]) : withoutPrevious,
      };
    });
  };

  const submit = () => {
    submitAnswers(answers);
  };

  return (
    <motion.section
      data-testid="question-request-card"
      className="my-2 w-full max-w-2xl overflow-hidden rounded-2xl border border-accent/25 bg-surface-1 shadow-[0_18px_45px_rgba(0,0,0,0.16)]"
      initial={{ opacity: 0, y: 10, scale: 0.985 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ type: 'spring', stiffness: 350, damping: 30 }}
    >
      <header className="flex items-center justify-between gap-3 border-b border-border/60 bg-linear-to-r from-accent/12 via-accent/4 to-transparent px-4 py-3">
        <div className="flex items-center gap-2.5">
          <span className="grid h-8 w-8 place-items-center rounded-xl border border-accent/30 bg-accent/12 text-accent">
            <HelpCircle size={16} />
          </span>
          <div>
            <h3 className="text-sm font-semibold text-text-primary">{t('chat.questionRequestTitle')}</h3>
            <p className="text-[11px] text-text-tertiary">
              {t('chat.questionRequestCount', { count: String(request.questions.length) })}
            </p>
          </div>
        </div>
        {isAnswered && (
          <span className="inline-flex items-center gap-1 rounded-full border border-success/25 bg-success/10 px-2 py-1 text-[10px] font-medium text-success">
            <CheckCircle2 size={11} /> {t('chat.questionAnswered')}
          </span>
        )}
      </header>

      <div className="space-y-3 p-3 sm:p-4">
        {request.questions.map((question, questionIndex) => {
          const selected = answers[question.id] ?? [];
          const isMulti = question.type === 'multi_choice';
          const isChoice = isMulti || question.type === 'single_choice';
          return (
            <fieldset
              key={question.id}
              disabled={isAnswered}
              className="rounded-xl border border-border/70 bg-surface-0/70 p-3.5 disabled:opacity-70"
            >
              <legend className="sr-only">{question.question}</legend>
              <div className="mb-2.5 flex items-start gap-3">
                <span className="mt-0.5 grid h-5 w-5 shrink-0 place-items-center rounded-full bg-accent/12 text-[10px] font-semibold text-accent">
                  {questionIndex + 1}
                </span>
                <div>
                  <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-accent">
                    {question.header ?? question.title}
                  </p>
                  <p className="mt-0.5 text-sm font-medium leading-5 text-text-primary">{question.question}</p>
                  {question.why && <p className="mt-1 text-xs leading-4 text-text-tertiary">{question.why}</p>}
                </div>
              </div>

              {isChoice && question.options && (
                <div className="grid gap-2 sm:grid-cols-2">
                  {question.options.map((option) => {
                    const checked = selected.includes(option.label);
                    return (
                      <button
                        key={option.label}
                        type="button"
                        role={isMulti ? 'checkbox' : 'radio'}
                        aria-checked={checked}
                        onClick={() => isMulti
                          ? toggleMulti(question.id, option.label)
                          : selectSingle(question.id, option.label, true)}
                        className={`relative rounded-lg border p-2.5 text-left transition ${
                          checked
                            ? 'border-accent/55 bg-accent/12 shadow-[0_0_0_1px_rgba(100,120,255,0.12)]'
                            : 'border-border bg-surface-2/65 hover:border-accent/35 hover:bg-surface-2'
                        }`}
                      >
                        <span className="flex items-start gap-2">
                          <span className={`mt-0.5 grid h-4 w-4 shrink-0 place-items-center border ${
                            isMulti ? 'rounded' : 'rounded-full'
                          } ${checked ? 'border-accent bg-accent text-white' : 'border-border bg-surface-1'}`}>
                            {checked && <Check size={10} />}
                          </span>
                          <span>
                            <span className="block text-xs font-medium text-text-primary">{option.label}</span>
                            {option.description && <span className="mt-0.5 block text-[11px] leading-4 text-text-tertiary">{option.description}</span>}
                          </span>
                        </span>
                      </button>
                    );
                  })}
                  <label className="rounded-lg border border-dashed border-border bg-surface-2/35 p-2.5 transition focus-within:border-accent/45">
                    <span className="block text-[10px] font-medium text-text-tertiary">{t('chat.questionOther')}</span>
                    <input
                      value={otherAnswers[question.id] ?? ''}
                      onChange={(event) => setOther(question.id, event.target.value, isMulti)}
                      placeholder={t('chat.questionOtherPlaceholder')}
                      className="mt-1 w-full bg-transparent text-xs text-text-primary outline-none placeholder:text-text-tertiary/70"
                    />
                  </label>
                </div>
              )}

              {(question.type === 'short' || question.type === 'confirm') && !isChoice && (
                question.type === 'confirm' ? (
                  <div className="flex gap-2">
                    {[t('chat.questionYes'), t('chat.questionNo')].map((label) => (
                      <button
                        key={label}
                        type="button"
                        onClick={() => selectSingle(question.id, label, true)}
                        className={`rounded-lg border px-4 py-2 text-xs font-medium transition ${selected.includes(label) ? 'border-accent bg-accent/12 text-accent' : 'border-border bg-surface-2 text-text-secondary hover:text-text-primary'}`}
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                ) : (
                  <input
                    value={selected[0] ?? ''}
                    onChange={(event) => selectSingle(question.id, event.target.value)}
                    placeholder={question.placeholder}
                    className="w-full rounded-lg border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary outline-none transition placeholder:text-text-tertiary focus:border-accent/55"
                  />
                )
              )}
              {question.type === 'long' && (
                <textarea
                  value={selected[0] ?? ''}
                  onChange={(event) => selectSingle(question.id, event.target.value)}
                  placeholder={question.placeholder}
                  rows={3}
                  className="w-full resize-y rounded-lg border border-border bg-surface-2 px-3 py-2 text-sm text-text-primary outline-none transition placeholder:text-text-tertiary focus:border-accent/55"
                />
              )}
            </fieldset>
          );
        })}
      </div>

      <footer className="flex items-center justify-between gap-3 border-t border-border/60 bg-surface-2/55 px-4 py-3">
        <p className="text-[11px] text-text-tertiary">{t('chat.questionResponseHint')}</p>
        <Button
          size="sm"
          variant="primary"
          icon={<Send size={13} />}
          disabled={!complete || isAnswered || !onSubmit}
          onClick={submit}
        >
          {isAnswered ? t('chat.questionAnswered') : t('chat.questionSubmit')}
        </Button>
      </footer>
    </motion.section>
  );
}

function hasCompleteAnswers(
  request: QuestionRequest,
  answers: Record<string, string[]>,
): boolean {
  return request.questions.every((question) => (
    (answers[question.id] ?? []).some((answer) => answer.trim())
  ));
}
