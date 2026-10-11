# perl scripts/deadline.pl SECONDS COMMAND...: run COMMAND in a new session. At the deadline (exit 124)
# or on Ctrl-C, kill each process below it and their process groups. When it ends, kill what is left of
# them. When this helper dies first (SIGKILL), the session's leader kills the same processes. A process
# that leaves its process group (setpgid or setsid) after its parent exits can still hold the output open.
use strict;
use warnings;
use POSIX ();

my $seconds = shift @ARGV;
# How often the leader checks for the command's exit and for the helper's death.
my $poll = 0.02;
my $helper = $$;
pipe(my $reader, my $writer) or die "deadline: cannot open a pipe: $!\n";
my $leader = fork;
defined $leader or die "deadline: cannot fork: $!\n";
lead() if $leader == 0;
close $writer;

my $caught = '';
$SIG{$_} = sub { $caught ||= $_[0]; stop() } for qw(ALRM INT TERM HUP);
alarm $seconds;
my $status = <$reader>;
alarm 0;
stop();
waitpid($leader, 0);
if ($caught eq 'ALRM') {
    print STDERR "deadline: @ARGV ran past its $seconds s deadline: it and the processes it started were killed\n";
    exit 124;
}
if ($caught) {
    $SIG{$caught} = 'DEFAULT';
    kill $caught, $$;
}
defined $status or die "deadline: the leader of @ARGV ended before the command did\n";
exit($status & 127 ? 128 + ($status & 127) : $status >> 8);

# The leader outlives the command, so its group id stays ours until the helper kills it. A helper that
# dies cannot kill it, so the leader polls getppid, which changes when the kernel gives it a new parent.
sub lead {
    close $reader;
    POSIX::setsid() > 0 or die "deadline: cannot start a session: $!\n";
    my $command = fork;
    defined $command or die "deadline: cannot fork: $!\n";
    if ($command == 0) {
        close $writer;
        exec { $ARGV[0] } @ARGV;
        exit 127;
    }
    # A write to a dead helper must not kill the leader before it kills the tree.
    $SIG{PIPE} = 'IGNORE';
    while (waitpid($command, POSIX::WNOHANG()) == 0) {
        orphaned() if getppid() != $helper;
        select(undef, undef, undef, $poll);
    }
    print {$writer} "$?\n";
    close $writer;
    select(undef, undef, undef, $poll) while getppid() == $helper;
    orphaned();
}

sub stop {
    my ($tree, $groups) = below($leader);
    kill 'KILL', @$tree;
    kill '-KILL', @$groups;
}

# The leader's own group holds the leader, so the leader signals that group last.
sub orphaned {
    my ($tree, $groups) = below($$);
    kill 'KILL', grep { $_ != $$ } @$tree;
    kill '-KILL', grep { $_ != $$ } @$groups;
    kill '-KILL', $$;
    exit 1;
}

# Return each process below $top, $top included, and each process group they are in: a child can start
# its own group.
sub below {
    my ($top) = @_;
    open my $ps, '-|', 'ps', '-A', '-o', 'pid=', '-o', 'ppid=', '-o', 'pgid='
        or die "deadline: cannot run ps: $!\n";
    my (%children, %group);
    while (my $line = <$ps>) {
        my ($pid, $ppid, $pgid) = split ' ', $line;
        push @{ $children{$ppid} }, $pid;
        $group{$pid} = $pgid;
    }
    close $ps or die "deadline: ps exited with status $?\n";
    my @tree = ($top);
    for (my $next = 0; $next < @tree; $next++) {
        push @tree, @{ $children{ $tree[$next] } || [] };
    }
    my %groups = map { $_ => 1 } $top, grep { defined } @group{@tree};
    return (\@tree, [keys %groups]);
}
