import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from 'react';
import {
  Check,
  ChevronLeft,
  ChevronRight,
  CircleHelp,
  Eye,
  EyeOff,
  Pencil,
  Send,
  Square,
} from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from '../../i18n';
import * as api from '../../lib/api';
import { interactionStore } from '../../lib/interactionStore';
import {
  formatQuestionResponse,
  type FormattedQuestionResponse,
  type QuestionRequest,
} from '../../lib/questionCards';
import { formatUserError } from '../../lib/userError';
import type {
  InteractionAnswers,
  InteractionDraft,
  InteractionQuestion,
  InteractionRequest,
} from '../../types/conversation';
import { Button } from '../ui/Button';

interface DecisionTrayProps {
  request: InteractionRequest;
  draft?: InteractionDraft;
  queuePosition: number;
  queueTotal: number;
  onSubmit: (response: FormattedQuestionResponse) => Promise<void>;
  onCancelTask: () => Promise<void>;
}

function asQuestionRequest(request: InteractionRequest): QuestionRequest {
  return {
    callId: request.toolCallId ?? request.interactionId,
    interactionId: request.interactionId,
    questions: request.questions.map((question) => ({
      id: question.id,
      title: question.header,
      header: question.header,
      question: question.question,
      why: question.why ?? undefined,
      type: question.type,
      options: question.options?.map((option) => ({
        label: option.label,
        description: option.description ?? undefined,
      })),
      placeholder: question.placeholder ?? undefined,
    })),
    status: 'pending',
  };
}

function questionIsComplete(question: InteractionQuestion, answers: InteractionAnswers): boolean {
  return (answers[question.id] ?? []).some((answer) => answer.trim().length > 0);
}

function requestIsComplete(request: InteractionRequest, answers: InteractionAnswers): boolean {
  return request.questions.every((question) => questionIsComplete(question, answers));
}

export function DecisionTray({
  request,
  draft,
  queuePosition,
  queueTotal,
  onSubmit,
  onCancelTask,
}: DecisionTrayProps) {
  const { t } = useTranslation();
  const answers = draft?.answers ?? {};
  const questionCount = request.questions.length;
  const rawIndex = draft?.currentQuestionIndex ?? 0;
  const currentIndex = Math.min(rawIndex, questionCount);
  const reviewing = currentIndex >= questionCount;
  const currentQuestion = request.questions[Math.min(currentIndex, questionCount - 1)];
  const recoveryPending = request.status === 'submitted' || request.status === 'acknowledged';
  const highRisk = request.kind === 'high_risk_confirmation';
  const modalRef = useRef<HTMLElement>(null);
  const [showAll, setShowAll] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [savedAnswers, setSavedAnswers] = useState<InteractionAnswers | null>(null);
  const [loadingSavedAnswers, setLoadingSavedAnswers] = useState(false);
  const submittedRef = useRef(false);

  useEffect(() => {
    if (!highRisk) return undefined;
    const previouslyFocused = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const frame = window.requestAnimationFrame(() => {
      const firstControl = modalRef.current?.querySelector<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
      );
      (firstControl ?? modalRef.current)?.focus();
    });
    return () => {
      window.cancelAnimationFrame(frame);
      if (previouslyFocused?.isConnected) previouslyFocused.focus();
    };
  }, [highRisk]);

  const trapModalFocus = (event: KeyboardEvent<HTMLElement>) => {
    if (!highRisk || event.key !== 'Tab' || !modalRef.current) return;
    const controls = Array.from(modalRef.current.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ));
    if (controls.length === 0) {
      event.preventDefault();
      modalRef.current.focus();
      return;
    }
    const first = controls[0];
    const last = controls[controls.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || !modalRef.current.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  };
  const partialMarkedRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    setSavedAnswers(null);
    if (!recoveryPending) return () => { disposed = true; };
    setLoadingSavedAnswers(true);
    void api.getInteractionResponse(request.interactionId)
      .then((response) => {
        if (!disposed) setSavedAnswers(response.answers);
      })
      .catch((error) => {
        if (!disposed) toast.error(formatUserError(t('chat.retry'), error));
      })
      .finally(() => {
        if (!disposed) setLoadingSavedAnswers(false);
      });
    return () => { disposed = true; };
  }, [recoveryPending, request.interactionId, t]);

  useEffect(() => {
    submittedRef.current = false;
    partialMarkedRef.current = request.status === 'partially_answered';
    setSubmitting(false);
    setCancelling(false);
    setShowAll(false);
    if (request.status !== 'pending') return;
    void api.markInteractionPresented(request.interactionId)
      .then((updated) => interactionStore.upsertRequest(updated))
      .catch(() => {});
  }, [request.interactionId, request.status]);

  const persist = (nextAnswers: InteractionAnswers, nextIndex = currentIndex) => {
    interactionStore.setDraft(request.interactionId, nextAnswers, nextIndex);
    if (!partialMarkedRef.current && Object.values(nextAnswers).some((values) => values.length > 0)) {
      partialMarkedRef.current = true;
      void api.markInteractionPartiallyAnswered(request.interactionId)
        .then((updated) => interactionStore.upsertRequest(updated))
        .catch(() => {});
    }
  };

  const submit = async (nextAnswers = answers) => {
    if (
      submittedRef.current
      || submitting
      || !requestIsComplete(request, nextAnswers)
    ) return;
    submittedRef.current = true;
    setSubmitting(true);
    try {
      await onSubmit(formatQuestionResponse(asQuestionRequest(request), nextAnswers));
      interactionStore.clearDraft(request.interactionId);
    } catch (error) {
      submittedRef.current = false;
      setSubmitting(false);
      toast.error(formatUserError(t('chat.questionSubmit'), error));
    }
  };

  const retrySavedResponse = async () => {
    if (submittedRef.current || submitting || loadingSavedAnswers) return;
    let nextAnswers = savedAnswers;
    if (!nextAnswers) {
      setLoadingSavedAnswers(true);
      try {
        nextAnswers = (await api.getInteractionResponse(request.interactionId)).answers;
        setSavedAnswers(nextAnswers);
      } catch (error) {
        toast.error(formatUserError(t('chat.retry'), error));
        return;
      } finally {
        setLoadingSavedAnswers(false);
      }
    }
    await submit(nextAnswers);
  };

  const advance = (nextAnswers = answers) => {
    if (!currentQuestion || !questionIsComplete(currentQuestion, nextAnswers)) return;
    persist(nextAnswers, Math.min(currentIndex + 1, questionCount));
  };

  const selectSingle = (value: string) => {
    if (!currentQuestion) return;
    const nextAnswers = { ...answers, [currentQuestion.id]: [value] };
    if (questionCount === 1 && currentQuestion.type === 'confirm') {
      persist(nextAnswers, questionCount);
      void submit(nextAnswers);
      return;
    }
    persist(nextAnswers, Math.min(currentIndex + 1, questionCount));
  };

  const toggleMulti = (value: string) => {
    if (!currentQuestion) return;
    const selected = answers[currentQuestion.id] ?? [];
    persist({
      ...answers,
      [currentQuestion.id]: selected.includes(value)
        ? selected.filter((answer) => answer !== value)
        : [...selected, value],
    });
  };

  const setTextAnswer = (value: string, multi = false) => {
    if (!currentQuestion) return;
    const optionLabels = new Set(currentQuestion.options?.map((option) => option.label) ?? []);
    const fixed = (answers[currentQuestion.id] ?? []).filter((answer) => optionLabels.has(answer));
    persist({
      ...answers,
      [currentQuestion.id]: value.trim()
        ? (multi ? [...fixed, value] : [value])
        : fixed,
    });
  };

  const selected = currentQuestion ? answers[currentQuestion.id] ?? [] : [];
  const optionLabels = useMemo(
    () => new Set(currentQuestion?.options?.map((option) => option.label) ?? []),
    [currentQuestion],
  );
  const customAnswer = selected.find((answer) => !optionLabels.has(answer)) ?? '';

  const cancelTask = async () => {
    if (cancelling) return;
    setCancelling(true);
    try {
      await onCancelTask();
      interactionStore.clearDraft(request.interactionId);
    } catch (error) {
      setCancelling(false);
      toast.error(formatUserError(t('chat.decisionTrayCancelTask'), error));
    }
  };

  return (
    <>
    {highRisk && (
      <div
        data-testid="decision-tray-modal-backdrop"
        aria-hidden="true"
        className="fixed inset-0 z-[89] bg-black/55 backdrop-blur-sm"
      />
    )}
    <section
      ref={modalRef}
      data-testid="decision-tray"
      aria-label={t('chat.questionRequestTitle')}
      role={highRisk ? 'alertdialog' : undefined}
      aria-modal={highRisk || undefined}
      tabIndex={highRisk ? -1 : undefined}
      onKeyDown={trapModalFocus}
      className={highRisk
        ? 'fixed left-1/2 top-1/2 z-[90] max-h-[85vh] w-[min(42rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-2xl border border-danger/45 bg-surface-1 shadow-[0_24px_80px_rgba(0,0,0,0.5)]'
        : 'z-40 mx-4 mb-2 shrink-0 overflow-hidden rounded-2xl border border-accent/35 bg-surface-1/98 shadow-[0_18px_50px_rgba(0,0,0,0.28)] backdrop-blur-xl max-sm:fixed max-sm:inset-x-0 max-sm:bottom-0 max-sm:mx-0 max-sm:mb-0 max-sm:max-h-[78vh] max-sm:overflow-y-auto max-sm:rounded-b-none'}
    >
      <header className="flex items-start justify-between gap-3 border-b border-border/60 bg-linear-to-r from-accent/14 via-accent/5 to-transparent px-4 py-3">
        <div className="flex min-w-0 items-start gap-2.5">
          <span className="grid h-8 w-8 shrink-0 place-items-center rounded-xl border border-accent/30 bg-accent/12 text-accent">
            <CircleHelp size={17} />
          </span>
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="truncate text-sm font-semibold text-text-primary">{request.title}</h2>
              {queueTotal > 1 && (
                <span className="rounded-full border border-border/70 bg-surface-2 px-2 py-0.5 text-[10px] font-medium text-text-tertiary">
                  {t('chat.decisionTrayQueue', { current: queuePosition, total: queueTotal })}
                </span>
              )}
            </div>
            <p className="mt-0.5 text-[11px] text-text-tertiary">
              {recoveryPending
                ? t('chat.questionResponseHint')
                : reviewing
                ? t('chat.decisionTrayReview')
                : t('chat.decisionTrayProgress', { current: currentIndex + 1, total: questionCount })}
            </p>
          </div>
        </div>
        {!recoveryPending && <button
          type="button"
          onClick={() => setShowAll((value) => !value)}
          className="inline-flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-text-tertiary transition-colors hover:bg-surface-2 hover:text-text-primary"
        >
          {showAll ? <EyeOff size={12} /> : <Eye size={12} />}
          {t(showAll ? 'chat.decisionTrayHideAll' : 'chat.decisionTrayShowAll')}
        </button>}
      </header>

      {showAll && !reviewing && (
        <div className="border-b border-border/60 bg-surface-0/50 px-4 py-2.5">
          <ol className="grid gap-1.5 sm:grid-cols-2">
            {request.questions.map((question, index) => (
              <li key={question.id} className="flex min-w-0 items-center gap-2 text-[11px] text-text-secondary">
                <span className={`grid h-4 w-4 shrink-0 place-items-center rounded-full text-[9px] ${
                  questionIsComplete(question, answers)
                    ? 'bg-success/15 text-success'
                    : index === currentIndex
                      ? 'bg-accent/15 text-accent'
                      : 'bg-surface-3 text-text-tertiary'
                }`}>
                  {questionIsComplete(question, answers) ? <Check size={9} /> : index + 1}
                </span>
                <span className="truncate">{question.header}</span>
              </li>
            ))}
          </ol>
        </div>
      )}

      <div className="px-4 py-3.5">
        {recoveryPending ? (
          <div data-testid="decision-tray-recovery" className="space-y-2">
            {request.questions.map((question) => (
              <div
                key={question.id}
                className="flex w-full items-start gap-3 rounded-xl border border-border/70 bg-surface-0/65 px-3 py-2.5"
              >
                <span className="mt-0.5 grid h-5 w-5 shrink-0 place-items-center rounded-full bg-success/12 text-success">
                  <Check size={11} />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-[10px] font-semibold uppercase tracking-[0.11em] text-text-tertiary">{question.header}</span>
                  <span className="mt-0.5 block text-xs text-text-primary">
                    {savedAnswers ? (savedAnswers[question.id] ?? []).join(', ') : t('common.loading')}
                  </span>
                </span>
              </div>
            ))}
          </div>
        ) : reviewing ? (
          <div data-testid="decision-tray-review" className="space-y-2">
            {request.questions.map((question, index) => (
              <button
                key={question.id}
                type="button"
                onClick={() => persist(answers, index)}
                className="flex w-full items-start gap-3 rounded-xl border border-border/70 bg-surface-0/65 px-3 py-2.5 text-left transition-colors hover:border-accent/35 hover:bg-surface-2/70"
              >
                <span className="mt-0.5 grid h-5 w-5 shrink-0 place-items-center rounded-full bg-success/12 text-success">
                  <Check size={11} />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-[10px] font-semibold uppercase tracking-[0.11em] text-text-tertiary">{question.header}</span>
                  <span className="mt-0.5 block truncate text-xs text-text-primary">{(answers[question.id] ?? []).join(', ')}</span>
                </span>
                <Pencil size={13} className="mt-1 shrink-0 text-text-tertiary" />
              </button>
            ))}
          </div>
        ) : currentQuestion ? (
          <fieldset>
            <legend className="sr-only">{currentQuestion.question}</legend>
            <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-accent">{currentQuestion.header}</p>
            <p className="mt-1 text-sm font-medium leading-5 text-text-primary">{currentQuestion.question}</p>
            {currentQuestion.why && <p className="mt-1 text-xs leading-4 text-text-tertiary">{currentQuestion.why}</p>}

            {(currentQuestion.type === 'single_choice' || currentQuestion.type === 'multi_choice') && (
              <div className="mt-3 grid gap-2 sm:grid-cols-2">
                {currentQuestion.options?.map((option) => {
                  const checked = selected.includes(option.label);
                  const multi = currentQuestion.type === 'multi_choice';
                  return (
                    <button
                      key={option.label}
                      type="button"
                      role={multi ? 'checkbox' : 'radio'}
                      aria-checked={checked}
                      onClick={() => multi ? toggleMulti(option.label) : selectSingle(option.label)}
                      className={`rounded-xl border px-3 py-2.5 text-left transition-colors ${
                        checked
                          ? 'border-accent/55 bg-accent/12'
                          : 'border-border/70 bg-surface-0/65 hover:border-accent/35 hover:bg-surface-2'
                      }`}
                    >
                      <span className="flex items-start gap-2">
                        <span className={`mt-0.5 grid h-4 w-4 shrink-0 place-items-center border ${multi ? 'rounded' : 'rounded-full'} ${checked ? 'border-accent bg-accent text-white' : 'border-border bg-surface-1'}`}>
                          {checked && <Check size={10} />}
                        </span>
                        <span className="min-w-0">
                          <span className="block text-xs font-medium text-text-primary">{option.label}</span>
                          {option.description && <span className="mt-0.5 block text-[11px] leading-4 text-text-tertiary">{option.description}</span>}
                        </span>
                      </span>
                    </button>
                  );
                })}
                <label className="rounded-xl border border-dashed border-border/80 bg-surface-0/45 px-3 py-2.5 transition-colors focus-within:border-accent/50">
                  <span className="block text-[10px] font-medium text-text-tertiary">{t('chat.questionOther')}</span>
                  <input
                    value={customAnswer}
                    onChange={(event) => setTextAnswer(event.target.value, currentQuestion.type === 'multi_choice')}
                    placeholder={t('chat.questionOtherPlaceholder')}
                    className="mt-1 w-full bg-transparent text-xs text-text-primary outline-none placeholder:text-text-tertiary/70"
                  />
                </label>
              </div>
            )}

            {currentQuestion.type === 'confirm' && (
              <div className="mt-3 flex gap-2">
                {[t('chat.questionYes'), t('chat.questionNo')].map((label) => (
                  <button
                    key={label}
                    type="button"
                    onClick={() => selectSingle(label)}
                    className={`rounded-lg border px-5 py-2 text-xs font-medium transition-colors ${selected.includes(label) ? 'border-accent bg-accent/12 text-accent' : 'border-border bg-surface-0 text-text-secondary hover:text-text-primary'}`}
                  >
                    {label}
                  </button>
                ))}
              </div>
            )}

            {(currentQuestion.type === 'short' || currentQuestion.type === 'long') && (
              <textarea
                autoFocus
                rows={currentQuestion.type === 'long' ? 4 : 2}
                value={selected[0] ?? ''}
                onChange={(event) => setTextAnswer(event.target.value)}
                onKeyDown={(event) => {
                  if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
                    event.preventDefault();
                    advance();
                  }
                }}
                placeholder={currentQuestion.placeholder ?? undefined}
                className="mt-3 w-full resize-y rounded-xl border border-border bg-surface-0 px-3 py-2.5 text-sm text-text-primary outline-none transition-colors placeholder:text-text-tertiary focus:border-accent/55"
              />
            )}
          </fieldset>
        ) : null}
      </div>

      <footer className="flex flex-wrap items-center justify-between gap-2 border-t border-border/60 bg-surface-2/55 px-4 py-3">
        <div className="flex items-center gap-2">
          {!recoveryPending && <Button
            size="sm"
            variant="ghost"
            icon={<Square size={12} />}
            disabled={cancelling || submitting}
            onClick={() => void cancelTask()}
          >
            {t('chat.decisionTrayCancelTask')}
          </Button>}
          <span className="hidden text-[11px] text-text-tertiary sm:inline">
            {recoveryPending ? t('chat.questionResponseHint') : t('chat.decisionTraySupplementHint')}
          </span>
        </div>
        <div className="flex items-center gap-2">
          {recoveryPending ? (
            <Button
              size="sm"
              variant="primary"
              icon={<Send size={13} />}
              disabled={submitting || loadingSavedAnswers}
              onClick={() => void retrySavedResponse()}
            >
              {t('chat.retry')}
            </Button>
          ) : currentIndex > 0 && (
            <Button
              size="sm"
              variant="ghost"
              icon={<ChevronLeft size={13} />}
              disabled={submitting}
              onClick={() => persist(answers, Math.max(0, currentIndex - 1))}
            >
              {t('common.edit')}
            </Button>
          )}
          {!recoveryPending && (reviewing ? (
              <Button
                size="sm"
                variant="primary"
                icon={<Send size={13} />}
                disabled={!requestIsComplete(request, answers) || submitting}
                onClick={() => void submit()}
              >
                {t('chat.questionSubmit')}
              </Button>
            ) : currentQuestion && !['single_choice', 'confirm'].includes(currentQuestion.type) ? (
              <Button
                size="sm"
                variant="primary"
                icon={<ChevronRight size={13} />}
                disabled={!questionIsComplete(currentQuestion, answers) || submitting}
                onClick={() => advance()}
              >
                {currentIndex === questionCount - 1
                    ? t('chat.decisionTrayReview')
                    : t('chat.decisionTrayContinue')}
              </Button>
            ) : null)}
        </div>
      </footer>
    </section>
    </>
  );
}
