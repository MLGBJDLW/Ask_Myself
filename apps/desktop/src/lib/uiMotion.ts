export const SOFT_EASE = [0.16, 1, 0.3, 1] as const;
export const INSTANT_TRANSITION = { duration: 0 } as const;

export const SOFT_FADE_TRANSITION = {
  opacity: { duration: 0.14, ease: 'easeOut' },
  y: { duration: 0.16, ease: 'easeOut' },
} as const;

export const SOFT_COLLAPSE_TRANSITION = {
  height: { duration: 0.18, ease: SOFT_EASE },
  opacity: { duration: 0.12, ease: 'easeOut' },
  y: { duration: 0.14, ease: 'easeOut' },
} as const;

export const SOFT_DROPDOWN_TRANSITION = {
  opacity: { duration: 0.12, ease: 'easeOut' },
  y: { duration: 0.14, ease: 'easeOut' },
} as const;

export function getSoftCollapseMotion(shouldReduceMotion: boolean, yOffset = -3) {
  if (shouldReduceMotion) {
    return {
      initial: false,
      animate: { opacity: 1 },
      exit: { opacity: 0 },
      transition: INSTANT_TRANSITION,
    } as const;
  }

  return {
    initial: { height: 0, opacity: 0, y: yOffset },
    animate: { height: 'auto', opacity: 1, y: 0 },
    exit: { height: 0, opacity: 0, y: yOffset },
    transition: SOFT_COLLAPSE_TRANSITION,
  } as const;
}

export function getSoftDropdownMotion(shouldReduceMotion: boolean, yOffset = -4) {
  if (shouldReduceMotion) {
    return {
      initial: false,
      animate: { opacity: 1 },
      exit: { opacity: 0 },
      transition: INSTANT_TRANSITION,
    } as const;
  }

  return {
    initial: { opacity: 0, y: yOffset },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, y: yOffset },
    transition: SOFT_DROPDOWN_TRANSITION,
  } as const;
}
